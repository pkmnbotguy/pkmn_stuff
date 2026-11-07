"""Network definition, weight loading and acting helpers (transformer policy / value net)."""

import paths
import json
import os

import torch
import torch.nn as nn
import torch.nn.functional as F


LSM = torch.log_softmax


MON_F, N_MON, MON_I = 80, 8, 7


class Net(nn.Module):
    def __init__(self, vocab, n_act, obs_dim, emb=(32, 16, 16, 24), mon_hidden=128, trunk=512, head=256):
        super().__init__()
        es, ea, ei, em = emb
        self.e_species = nn.Embedding(vocab["species"], es)
        self.e_ability = nn.Embedding(vocab["ability"], ea)
        self.e_item = nn.Embedding(vocab["item"], ei)
        self.e_move = nn.Embedding(vocab["move"], em)
        self.em = em
        mon_in = MON_F + es + ea + ei + em
        self.mon = nn.Sequential(nn.Linear(mon_in, mon_hidden), nn.ReLU(), nn.Linear(mon_hidden, mon_hidden), nn.ReLU())
        self.obs_dim = obs_dim
        self.field_dim = obs_dim - N_MON * MON_F
        self.trunk = nn.Sequential(nn.Linear(N_MON * mon_hidden + self.field_dim, trunk), nn.ReLU(),
                                   nn.Linear(trunk, trunk // 2), nn.ReLU())
        head_in = trunk // 2 + mon_hidden + 4 * em
        self.heads = nn.ModuleList([nn.Sequential(nn.Linear(head_in, head), nn.ReLU(), nn.Linear(head, n_act)) for _ in range(2)])
        self.value = nn.Sequential(nn.Linear(trunk // 2, 128), nn.ReLU(), nn.Linear(128, 1))
        # opponent prediction: each opposing slot's action (their numbering), from our view
        self.opp_heads = nn.ModuleList([nn.Sequential(nn.Linear(head_in, head), nn.ReLU(), nn.Linear(head, n_act)) for _ in range(2)])
        # team preview head: 12 Pokemon (own 6, then theirs; same encoder) -> 90 options (which 4, which 2 lead)
        hh = mon_hidden
        self.pv_q = nn.Sequential(nn.Linear(3 * hh, hh), nn.ReLU(), nn.Linear(hh, hh), nn.ReLU())
        self.pv_score = nn.Sequential(nn.Linear(4 * hh, 256), nn.ReLU(), nn.Linear(256, 1))
        self.pv_value = nn.Sequential(nn.Linear(3 * hh, 128), nn.ReLU(), nn.Linear(128, 1))
        four, leads = preview_index()
        # option x Pokemon membership (0/1): which 4 are brought, which 2 lead
        w4 = torch.zeros(len(four), 6)
        w2 = torch.zeros(len(four), 6)
        for k, (f4, l2) in enumerate(zip(four, leads)):
            w4[k, f4] = 1.0
            w2[k, l2] = 1.0
        self.register_buffer("pv_w4", w4, persistent=False)
        self.register_buffer("pv_w2", w2, persistent=False)

    def _mons(self, mf, ids):
        moves = self.e_move(ids[:, :, 3:7])                       # [b, n, 4, em]
        x = torch.cat([mf, self.e_species(ids[:, :, 0]), self.e_ability(ids[:, :, 1]), self.e_item(ids[:, :, 2]),
                       moves.mean(2)], -1)
        return self.mon(x), moves

    def preview(self, pf, pi):
        """Team preview: logits over the 90 options [b, 90] and the matchup's value [b]."""
        b = pf.shape[0]
        mf = pf.view(b, 12, MON_F)
        mon, _ = self._mons(mf, pi.view(b, 12, MON_I).long())
        present = mf[:, :, 0:1]
        mon = mon * present
        own, foe, op, fp = mon[:, :6], mon[:, 6:], present[:, :6], present[:, 6:]
        foe_mean = foe.sum(1) / fp.sum(1).clamp(min=1)
        foe_max = foe.masked_fill(fp == 0, -1e4).max(1).values
        foe_max = torch.where(fp.sum(1) > 0, foe_max, torch.zeros_like(foe_max))
        q = self.pv_q(torch.cat([own, foe_mean.unsqueeze(1).expand(-1, 6, -1), foe_max.unsqueeze(1).expand(-1, 6, -1)], -1)) * op
        q_mean = q.sum(1) / op.sum(1).clamp(min=1)
        # = pv_score(cat[sum of the 4 brought, sum of the 2 leads, context]) per option, computed without
        # materialising [b, 90, 4h]: the first layer is linear, so project each Pokemon once and add up
        lin1, lin2 = self.pv_score[0], self.pv_score[2]
        hh = q.shape[-1]
        w = lin1.weight                                            # [256, 4h]
        p4 = q @ w[:, :hh].t()                                     # [b, 6, 256]
        p2 = q @ w[:, hh:2 * hh].t()
        pc = torch.cat([q_mean, foe_mean], -1) @ w[:, 2 * hh:].t() + lin1.bias   # [b, 256]
        z = torch.relu(torch.einsum("ok,bkd->bod", self.pv_w4, p4) + torch.einsum("ok,bkd->bod", self.pv_w2, p2) + pc.unsqueeze(1))
        logits = lin2(z).squeeze(-1)                               # [b, 90]
        value = self.pv_value(torch.cat([q_mean, foe_mean, foe_max], -1)).squeeze(-1)
        return logits, value

    def forward(self, obs_f, obs_i, opp=False, aux=False):
        """Our action logits [b, 2, A] and the value [b]; with opp=True also the predicted opponent action
        logits [b, 2, A] (their slots, their numbering)."""
        b = obs_f.shape[0]
        if obs_f.shape[1] > self.obs_dim:
            obs_f = obs_f[:, : self.obs_dim]          # a net made before newer features were appended
        mf = obs_f[:, : N_MON * MON_F].view(b, N_MON, MON_F)
        ids = obs_i.view(b, N_MON, MON_I).long()
        mon, moves = self._mons(mf, ids)                           # [b, 8, h], [b, 8, 4, em]
        h = self.trunk(torch.cat([mon.reshape(b, -1), obs_f[:, N_MON * MON_F:]], -1))
        logits = []
        for k in range(2):
            w = mf[:, :4, 4 + k]                                   # which of our Pokemon is in slot k
            act_mon = (w.unsqueeze(-1) * mon[:, :4]).sum(1)
            act_moves = (w[:, :, None, None] * moves[:, :4]).sum(1).reshape(b, 4 * self.em)
            logits.append(self.heads[k](torch.cat([h, act_mon, act_moves], -1)))
        if opp:
            ol = []
            for j in range(2):
                w = mf[:, 4:, 4 + j]                               # which of theirs is in their slot j
                fm = (w.unsqueeze(-1) * mon[:, 4:]).sum(1)
                fmv = (w[:, :, None, None] * moves[:, 4:]).sum(1).reshape(b, 4 * self.em)
                ol.append(self.opp_heads[j](torch.cat([h, fm, fmv], -1)))
            out = (torch.stack(logits, 1), self.value(h).squeeze(-1), torch.stack(ol, 1))
        else:
            out = (torch.stack(logits, 1), self.value(h).squeeze(-1))   # [b, 2, A], [b]
        return out + ((None,) if not opp else ()) + (None,) if aux else out


AMP = {"on": False}


_FAST_BF16 = {}


def fast_bf16():
    """(2026-10-18) bf16 autocast only on GPUs with native bf16 math (compute capability 8.0+: Ampere, Ada, Blackwell). On
    the GTX 1070 (6.1) it was 25-370% SLOWER than fp32 and less precise (agent / review turned it on with any GPU)."""
    if "ok" not in _FAST_BF16:
        try:
            _FAST_BF16["ok"] = torch.cuda.is_available() and torch.cuda.get_device_capability()[0] >= 8
        except Exception:
            _FAST_BF16["ok"] = False
    return _FAST_BF16["ok"]


def amp(graph=False):
    """bf16 autocast on GPUs with fast bf16. graph=True for CUDA-graph CAPTURE (2026-10-24, first v2 rental): autocast's
    weight-cast cache is freed when the autocast block ends, so a graph captured with it on replays reads of freed memory
    ("illegal memory access" in fastinfer's graphs on the 5090; the 1070 never autocasts, so home tests couldn't see it)."""
    import contextlib
    if AMP["on"] and torch.cuda.is_available() and (fast_bf16() or os.environ.get("VGC_FORCE_BF16") == "1"):
        return torch.autocast(device_type="cuda", dtype=torch.bfloat16, cache_enabled=not graph)
    return contextlib.nullcontext()


FP8 = {"emulate": False, "row": True}


_E4M3_MAX = 448.0


def fp8_linear(x, w8, s8, bias):
    shp = x.shape
    x2 = x.reshape(-1, shp[-1])
    xf = x2.float()
    if FP8["row"]:
        xs = (xf.abs().amax(1, keepdim=True) / _E4M3_MAX).clamp(min=1e-12)        # [M, 1]
        sb = s8.view(1, -1)                                                         # [1, N]
    else:
        xs = (xf.abs().amax() / _E4M3_MAX).clamp(min=1e-12)
        sb = s8[0]
    x8 = (xf / xs).clamp(-_E4M3_MAX, _E4M3_MAX).to(torch.float8_e4m3fn)
    if FP8["emulate"]:
        y = (x8.float() * xs) @ (w8.float() * s8.view(-1, 1)).t()
        if bias is not None:
            y = y + bias.float()
        y = y.to(x.dtype)
    else:
        y = torch._scaled_mm(x8, w8.t(), scale_a=xs, scale_b=sb,
                             bias=None if bias is None else bias.to(torch.bfloat16), out_dtype=torch.bfloat16).to(x.dtype)
    return y.reshape(*shp[:-1], y.shape[-1])


def preview_index():
    """The 90 team preview options in the engine's order (vgcsim.preview_options): (fours, lead pairs)."""
    import itertools
    fours, leads = [], []
    for four in itertools.combinations(range(6), 4):
        for x, y in itertools.combinations(range(4), 2):
            fours.append(list(four))
            leads.append([four[x], four[y]])
    return fours, leads


def arch_of(sd):
    """The network kind and size a checkpoint's weights were made with (read from the layer shapes)."""
    if "tok_mon.weight" in sd:
        layers = len({k.split(".")[2] for k in sd if k.startswith("enc.layers.")})
        a = {"kind": "transformer", "d_model": sd["tok_mon.weight"].shape[0], "layers": layers,
             "ffn": sd["enc.layers.0.linear1.weight"].shape[0],
             "tokens": 13 if "dec.layers.0.self_attn.in_proj_weight" in sd else 25 if "tok_foe_move.weight" in sd else 17}
        if a["tokens"] == 13:
            a["obs_dim"] = OBS_FLAG
            a["dec_layers"] = len({k.split(".")[2] for k in sd if k.startswith("dec.layers.")})
        if "tok_hist.weight" in sd:
            a["history"] = True
        if "aux_fhp.weight" in sd:
            a["future"] = True
        if "jt_a.weight" in sd:
            a["joint"] = True
        if "aux_bel.weight" in sd:
            a["belief"] = True
        if "tok_unc_foe.weight" in sd:
            a["unc"] = True
        if "tok_ext.weight" in sd:
            a["ext"] = True
        if "pv_pair_a.weight" in sd:
            a["pvpair"] = True
        if "oj_a.weight" in sd:
            a["oppjoint"] = True
        if "act_id" in sd:
            a["activation"] = ACTS[int(sd["act_id"]) - 1]
        # (2026-11-05, v3)
        if "vbins_n" in sd:
            a["vbins"] = int(sd["vbins_n"])
        if "enc_opts_b" in sd:
            a["enc_opts"] = int(sd["enc_opts_b"])
        if "softcap_v" in sd:
            a["softcap"] = float(sd["softcap_v"])
        if "oc_proj.weight" in sd:
            a["oppcond"] = True
        if "tb_lin.weight" in sd:
            a["typebias"] = True
        if "aux_fate.weight" in sd:
            a["v3heads"] = True
        if "bel_in.weight" in sd:
            a["belin"] = True
            a["obs_dim"] = OBS_BEL
        if "mem_in.weight" in sd:
            a["memory"] = True
            a["obs_dim"] = MEM0 + MEM_W
        return a
    mh = sd["mon.0.weight"].shape[0]
    return {"obs_dim": sd["trunk.0.weight"].shape[1] - N_MON * mh + N_MON * MON_F, "mon_hidden": mh,
            "trunk": sd["trunk.0.weight"].shape[0], "head": sd["heads.0.0.weight"].shape[0]}


def load_weights(net, sd):
    """Load a checkpoint's weights; nets saved before the preview head existed are fine (its weights
    stay freshly initialised). Returns the names of the parameters that were missing."""
    missing, unexpected = net.load_state_dict(sd, strict=False)
    bad = [k for k in missing if not k.startswith(("pv_", "opp_", "aux_", "tok_hist", "jt_", "bel_", "tok_unc", "unc_", "tok_ext", "ext_", "oj_", "act_id", "oc_", "tb_", "mem_"))] + list(unexpected)
    if bad:
        raise RuntimeError(f"checkpoint doesn't match the network: {bad[:5]}")
    return list(missing)


OBS_T = 776


HIST_F = 20


UNC0, UNC_F = 936, 68


EXT0, EXT_F = 1004, 68


FIELD_F, DMG0 = 40, 680


V3_0, V3_F = 1548, 87


MEM0, MEM_K, MEM_F = 2468, 16, 78


MEM_W = MEM_K * MEM_F


OBS_V3 = V3_0 + 8 * V3_F


FLAG0, FLAG_F = OBS_V3, 4


OBS_FLAG = FLAG0 + 16 * FLAG_F


BEL0, BEL_F = OBS_FLAG, 16


OBS_BEL = BEL0 + 10 * BEL_F


ACTS = ("relu", "gelu", "gelu_tanh", "silu")


HLG_MAX = 1.1


def hlg_centers(n):
    e = torch.linspace(-HLG_MAX, HLG_MAX, n + 1)
    return (e[:-1] + e[1:]) / 2


def hlg_mean(logits, centers):
    return (torch.softmax(logits.float(), -1) * centers).sum(-1).clamp(-1, 1)


def _sdpa_ctx():
    """Context forcing the SDPA kernel named by TNET_SDPA_BACKEND (TNET_ATTN=fused); a no-op context when unset."""
    import contextlib
    name = os.environ.get("TNET_SDPA_BACKEND", "")
    if not name:
        return contextlib.nullcontext()
    from torch.nn.attention import SDPBackend, sdpa_kernel
    return sdpa_kernel({"flash": SDPBackend.FLASH_ATTENTION, "cudnn": SDPBackend.CUDNN_ATTENTION,
                        "efficient": SDPBackend.EFFICIENT_ATTENTION, "math": SDPBackend.MATH}[name])


ACT_FN = {"relu": F.relu, "gelu": F.gelu, "gelu_tanh": lambda x: F.gelu(x, approximate="tanh"), "silu": F.silu}


class TNet(nn.Module):
    attn_impl = os.environ.get("TNET_ATTN", "sdpa")     # "manual": see _encode

    def __init__(self, vocab, n_act, d_model=256, layers=6, ffn=1024, emb=(32, 16, 16, 24), obs_dim=OBS_T, tokens=25,
                 history=False, activation="relu", dec_layers=2, vbins=0, enc_opts=0, softcap=0.0):
        """(2026-11-05, v3 options for FRESH nets; stored as buffers so arch_of reads them back)
          vbins:    HL-Gauss value head with that many bins (forward still returns the mean as the value; vdist=True returns
                    the bin logits instead, for the loss)
          enc_opts: encoder bits 1 QK-norm, 2 gated attention (sigmoid gate per head), 4 value residual (each layer's values
                    mixed with layer 0's, learned weight), 8 zero-init output projections (attention + feed-forward)
          softcap:  policy / opponent / preview logits = c tanh(logits / c) (0 = off)"""
        super().__init__()
        self.vbins, self.enc_opts, self.softcap = int(vbins), int(enc_opts), float(softcap)
        assert n_act == 38, "the transformer's action layout assumes 38 actions per slot"
        assert tokens in (25, 17, 13)
        # (2026-10-22) the encoder's feed-forward activation (relu = PyTorch's default and every net so far);
        # stored as a buffer so arch_of reads it from the weights (ACTS index + 1; absent = relu)
        assert activation in ACTS, activation
        self.register_buffer("act_id", torch.tensor(ACTS.index(activation) + 1))
        es, ea, ei, em = emb
        d = d_model
        self.obs_dim = max(obs_dim, OBS_FLAG) if tokens == 13 else obs_dim
        self.d = d
        # 17 tokens: no opponent move tokens; their damage on us (4 moves x our 2 slots x (fraction, KO))
        # goes into each active opponent Pokemon's token instead
        self.tokens = tokens
        self.extra = 16 if tokens in (17, 13) else 0
        self.e_species = nn.Embedding(vocab["species"], es)
        self.e_ability = nn.Embedding(vocab["ability"], ea)
        self.e_item = nn.Embedding(vocab["item"], ei)
        self.e_move = nn.Embedding(vocab["move"], em)
        self.tok_mon = nn.Linear(MON_F + es + ea + ei + em + self.extra, d)
        self.tok_my_move = nn.Linear(em + 4 + 2 + 1, d)      # move, damage vs foe a / b (frac, KO), vs partner, pp
        if tokens == 25:
            self.tok_foe_move = nn.Linear(em + 4, d)         # move, damage vs our slot a / b
        self.tok_field = nn.Linear(FIELD_F + 12 + 4, d)      # field, speed order, bench, unseen-STAB estimate
        self.kind = nn.Embedding(5, d)                       # our mon, foe mon, our move, foe move, field
        #                         (13 tokens: our brought, their obs-row Pokemon, our unbrought, their preview-only, field)
        self.pos = nn.Embedding(tokens, d)
        layer = nn.TransformerEncoderLayer(d, max(1, d // 64), ffn, dropout=0.0, batch_first=True, norm_first=True,
                                           activation=ACT_FN[activation])
        self.enc = nn.TransformerEncoder(layer, layers, enable_nested_tensor=False)
        if tokens == 13:
            self.register_buffer("_kinds13", torch.tensor([0, 0, 0, 0, 2, 2]), persistent=False)   # CUDA-graph safe constant
            # (2026-11-03, MysteriousWind's layout) 13 tokens = our 6 + their 6 (seen ones first) + field; no move tokens in
            # the encoder. Move vectors come from two small decoders that cross-attend to the encoder output: one query per
            # our active move (tok_my_move input + the slot's Pokemon), one per their active move (move id + damage on us +
            # the slot's Pokemon). The heads then read the same 25-row layout as the 25-token net (virtual rows).
            def dec():
                return nn.TransformerDecoder(nn.TransformerDecoderLayer(
                    d, max(1, d // 64), ffn, dropout=0.0, batch_first=True, norm_first=True,
                    activation=ACT_FN[activation]), dec_layers)
            self.dec, self.dec_norm, self.dec_pos = dec(), nn.LayerNorm(d), nn.Embedding(8, d)
            self.odec, self.odec_norm, self.odec_pos = dec(), nn.LayerNorm(d), nn.Embedding(8, d)
            self.odec_dmg = nn.Linear(4, d)                  # their move's damage on our slot a / b (fraction, KO)
            self.unseen_foe = nn.Parameter(torch.zeros(d))   # stands in for an obs foe row that has no token
            self.tok_flag = nn.Linear(FLAG_F, d)             # (2026-11-03) per-token flags (FLAG0 block)
        self.norm = nn.LayerNorm(d)
        self.no_target = nn.Parameter(torch.zeros(d))
        self.pol_move = nn.Sequential(nn.Linear(3 * d, d), nn.ReLU(), nn.Linear(d, 2))
        self.pol_switch = nn.Sequential(nn.Linear(2 * d, d), nn.ReLU(), nn.Linear(d, 1))
        self.pol_other = nn.Linear(d, 2)
        self.value = nn.Sequential(nn.Linear(2 * d, d), nn.ReLU(), nn.Linear(d, self.vbins if self.vbins else 1))
        # opponent prediction (their slots' actions, their numbering) from our view
        self.opp_mv = nn.Linear(em, d)
        self.opp_move = nn.Sequential(nn.Linear(3 * d, d), nn.ReLU(), nn.Linear(d, 2))
        self.opp_switch = nn.Sequential(nn.Linear(2 * d, d), nn.ReLU(), nn.Linear(d, 1))
        self.opp_other = nn.Linear(d, 2)
        # team preview: the same encoder over 12 Pokemon + a summary token
        self.pv_pos = nn.Embedding(13, d)
        self.pv_cls = nn.Parameter(torch.zeros(d))
        self.pv_p4 = nn.Linear(d, d, bias=False)
        self.pv_p2 = nn.Linear(d, d, bias=False)
        self.pv_ctx = nn.Linear(d, d)
        self.pv_out = nn.Linear(d, 1)
        self.pv_value = nn.Sequential(nn.Linear(d, d), nn.ReLU(), nn.Linear(d, 1))
        # auxiliary: will this Pokemon (token) faint before our next decision? (training signal only, the reward is unchanged)
        self.aux_ko = nn.Linear(d, 1)
        # history (2026-10-10): each Pokemon's 20 history features (last move slot / target, streak, Protects, uses this
        # battle...) added to its token. Registered LAST (keep_opt_state keeps the old parameters' Adam state) and
        # zero-initialised: a net converted from one without history plays exactly like it at first.
        self.history = history
        if history:
            self.tok_hist = nn.Linear(HIST_F, d)
            nn.init.zeros_(self.tok_hist.weight)
            nn.init.zeros_(self.tok_hist.bias)
            # effective weights = tok_hist x this factor: with Adam the layer then learns `factor` times faster (the
            # per-step change of a raw weight is ~lr whatever the gradient scale). A buffer, so checkpoints carry it;
            # history nets saved before it existed load with 1 (= their own parametrisation).
            self.register_buffer("tok_hist_scale", torch.tensor(1.0))
        four, leads = preview_index()
        w4 = torch.zeros(len(four), 6)
        w2 = torch.zeros(len(four), 6)
        for k, (f4, l2) in enumerate(zip(four, leads)):
            w4[k, f4] = 1.0
            w2[k, l2] = 1.0
        self.register_buffer("pv_w4", w4, persistent=False)
        self.register_buffer("pv_w2", w2, persistent=False)
        if tokens != 13:
            self.register_buffer("kinds", torch.tensor([0] * 4 + [1] * 4 + [2] * 8 + ([3] * 8 if tokens == 25 else []) + [4]),
                                 persistent=False)
        if self.vbins:
            self.register_buffer("vbins_n", torch.tensor(self.vbins))
            # bin edges on [-HLG_MAX, HLG_MAX] (a little past +-1, so a target at +-1 smeared by sigma keeps its mean)
            self.register_buffer("v_centers", hlg_centers(self.vbins), persistent=False)
        if self.enc_opts:
            self.register_buffer("enc_opts_b", torch.tensor(self.enc_opts))
            H = max(1, d // 64)
            if self.enc_opts & 1:
                self.qk_g = nn.Parameter(torch.ones(layers, 2, d // H))
            if self.enc_opts & 2:
                self.attn_gate = nn.ModuleList([nn.Linear(d, H) for _ in range(layers)])
            if self.enc_opts & 4:
                self.vres_a = nn.Parameter(torch.full((layers,), 0.5))
            if self.enc_opts & 8:
                for lay in self.enc.layers:
                    nn.init.zeros_(lay.self_attn.out_proj.weight)
                    nn.init.zeros_(lay.linear2.weight)
        if self.softcap:
            self.register_buffer("softcap_v", torch.tensor(self.softcap))

    def add_future_heads(self):
        """(2026-10-19) Future-prediction heads, training signal only (the reward is unchanged). Added after
        construction, so they are registered LAST (keep_opt_state keeps the old parameters' Adam state) and old checkpoints
        load (missing aux_ keys start fresh).
          aux_fhp   per Pokemon token: HP fraction lost before this row's next decision (heals negative; regression)
          aux_fsurv per Pokemon token: faints before our next 1 / 2 / 3 / 4 decisions, or survives longer (5 classes)
          aux_flen  field token: our decisions left in the game after this one, 0..7, 8+ (9 classes)
          aux_fwx   field token: weather (5) and terrain (5) at our next decision
          aux_fspd  field token: Trick Room up / our Tailwind up / their Tailwind up at our next decision (3 yes/no logits)
        Weather / terrain / speed labels that CHANGE before the next decision weigh more (most are just the visible
        countdowns)."""
        if hasattr(self, "aux_fhp"):
            if not hasattr(self, "aux_fspd"):          # heads made before the Trick Room / Tailwind labels
                self.aux_fspd = nn.Linear(self.aux_ko.in_features, 3).to(self.aux_ko.weight.device)
            return
        d = self.aux_ko.in_features
        dev = self.aux_ko.weight.device
        self.aux_fhp = nn.Linear(d, 1).to(dev)
        self.aux_fsurv = nn.Linear(d, 5).to(dev)
        self.aux_flen = nn.Linear(d, 9).to(dev)
        self.aux_fwx = nn.Linear(d, 10).to(dev)
        self.aux_fspd = nn.Linear(d, 3).to(dev)

    def _mon_x(self, mf, ids, extra=None):
        moves = self.e_move(ids[:, :, 3:7])
        parts = [mf, self.e_species(ids[:, :, 0]), self.e_ability(ids[:, :, 1]), self.e_item(ids[:, :, 2]), moves.mean(2)]
        if self.extra:
            parts.append(extra if extra is not None else mf.new_zeros(mf.shape[0], mf.shape[1], self.extra))
        return torch.cat(parts, -1)

    def add_joint_head(self, rank=32, width=64):
        """(2026-10-19) Joint policy over the two slots: logit(a1, a2) = l1[a1] + l2[a2] + I[a1, a2], with I a low-rank
        interaction between the two slots' action vectors, symmetric in the slot roles (no slot is 'first'):
        I = <A(x1), B(x2)> + <B(x1), A(x2)>. jt_a starts at zero, so a net that gets this head plays exactly like before
        (apart from the pair rules of joint_logprobs). Registered last (Adam state of the old weights kept)."""
        if hasattr(self, "jt_a"):
            return
        d = self.d
        dev = self.aux_ko.weight.device
        # action vector = W(move) + W(target) + W(mega) + W(slot's Pokemon) (+ bias), computed per part (the [b, 2, 38, .]
        # tensor is only built at the narrow `width`: the full-width version doubled the update time on an 8 GB card)
        self.jt_mega = nn.Parameter(torch.zeros(2, width, device=dev))
        self.jt_sw = nn.Parameter(torch.zeros(width, device=dev))
        self.jt_other = nn.Parameter(torch.zeros(2, width, device=dev))
        self.jt_mlp = nn.Linear(d, width).to(dev)
        self.jt_a = nn.Linear(width, rank).to(dev)
        self.jt_b = nn.Linear(width, rank).to(dev)
        nn.init.zeros_(self.jt_a.weight)
        nn.init.zeros_(self.jt_a.bias)
        # A and B outputs x this factor: with Adam the head then learns `factor` times faster than the rest of the net (as
        # tok_hist_scale). A buffer, so checkpoints carry it. A fresh head sets it.
        self.register_buffer("jt_scale", torch.tensor(1.0, device=dev))

    def add_preview_pair(self, rank=16, width=64):
        """(2026-10-22, rebuild item 2) Pairwise term in the team preview score. The option score was purely additive per
        Pokemon (sum of pv_p4 over the four + pv_p2 over the leads + a context term): nothing in the head could say "A and
        B together" beyond what the shared encoder squeezed into each token. Now score(option) also gets, over the 6 pairs
        inside the chosen four, <A(u_i), B(u_j)> + <B(u_i), A(u_j)>, and over the lead pair <C(u_i), D(u_j)> + <D(u_i), C(u_j)>
        (u = relu(pv_pair_q(token)), low rank, symmetric in the two Pokemon). A and C start at zero: a net that gets this
        term previews exactly as before. Registered last (Adam state of the old weights kept); 15 pairs x 90 options as
        constant membership tables, so the cost is negligible."""
        if hasattr(self, "pv_pair_a"):
            return
        d = self.d
        dev = self.aux_ko.weight.device
        self.pv_pair_q = nn.Linear(d, width).to(dev)
        self.pv_pair_a = nn.Linear(width, rank).to(dev)
        self.pv_pair_b = nn.Linear(width, rank).to(dev)
        self.pv_lead_a = nn.Linear(width, rank).to(dev)
        self.pv_lead_b = nn.Linear(width, rank).to(dev)
        for lin in (self.pv_pair_a, self.pv_lead_a):
            nn.init.zeros_(lin.weight)
            nn.init.zeros_(lin.bias)
        self.register_buffer("pv_pair_scale", torch.tensor(1.0, device=dev))
        four, leads = preview_index()
        pairs = [(i, j) for i in range(6) for j in range(i + 1, 6)]
        wp4 = torch.zeros(len(four), len(pairs))
        wp2 = torch.zeros(len(four), len(pairs))
        for k, (f4, l2) in enumerate(zip(four, leads)):
            for p, (i, j) in enumerate(pairs):
                if i in f4 and j in f4:
                    wp4[k, p] = 1.0
                if {i, j} == set(l2):
                    wp2[k, p] = 1.0
        self.register_buffer("pv_wp4", wp4.to(dev), persistent=False)
        self.register_buffer("pv_wp2", wp2.to(dev), persistent=False)
        self.register_buffer("pv_pi", torch.tensor([i for i, _ in pairs], device=dev), persistent=False)
        self.register_buffer("pv_pj", torch.tensor([j for _, j in pairs], device=dev), persistent=False)

    def _pv_pair(self, own, present6):
        """[b, 90] pairwise part of the preview score (add_preview_pair); absent Pokemon contribute nothing"""
        u = torch.relu(self.pv_pair_q(own)) * present6.unsqueeze(-1).to(own.dtype)      # [b, 6, w]
        sc = self.pv_pair_scale
        A, B = self.pv_pair_a(u) * sc, self.pv_pair_b(u) * sc
        C, D = self.pv_lead_a(u) * sc, self.pv_lead_b(u) * sc
        s4 = torch.einsum("bir,bjr->bij", A, B)
        s4 = s4 + s4.transpose(1, 2)
        s2 = torch.einsum("bir,bjr->bij", C, D)
        s2 = s2 + s2.transpose(1, 2)
        p4 = s4[:, self.pv_pi, self.pv_pj]                                              # [b, 15]
        p2 = s2[:, self.pv_pi, self.pv_pj]
        return p4 @ self.pv_wp4.t() + p2 @ self.pv_wp2.t()

    def add_opp_joint(self, rank=32, width=64):
        """(2026-10-22, rebuild item 3) Joint opponent prediction: the opponent head predicts each of their slots' actions
        separately (marginals), so the search's top-K opponent joint candidates were products of marginals: "Protect on A
        + attack with B" and "attack with A + Protect on B" got the same weight as the (rarely played) double Protect.
        logit(a1, a2) = ol1[a1] + ol2[a2] + I[a1, a2] with the same low-rank symmetric interaction as add_joint_head, over
        THEIR action vectors (their move x target x Mega, switch to their Pokemon j, other). oj_a starts at zero: the joint
        distribution is the product of the marginals at first. Registered last. Output: forward(opp_joint=True)."""
        if hasattr(self, "oj_a"):
            return
        d = self.d
        dev = self.aux_ko.weight.device
        self.oj_mega = nn.Parameter(torch.zeros(2, width, device=dev))
        self.oj_sw = nn.Parameter(torch.zeros(width, device=dev))
        self.oj_other = nn.Parameter(torch.zeros(2, width, device=dev))
        self.oj_mlp = nn.Linear(d, width).to(dev)
        self.oj_a = nn.Linear(width, rank).to(dev)
        self.oj_b = nn.Linear(width, rank).to(dev)
        nn.init.zeros_(self.oj_a.weight)
        nn.init.zeros_(self.oj_a.bias)
        self.register_buffer("oj_scale", torch.tensor(1.0, device=dev))

    def add_belief_head(self):
        """(2026-10-20) Belief head, training signal only: per opponent Pokemon token (obs rows 4-7) logits
        over species / abilities / items / moves = what that Pokemon really is / holds / knows, trained only where the row's
        own (closed-sheet) view doesn't show it (labels: the true sets; an unseen foe's token is padding for the
        attention keys but still reads the whole position). Moves: one distribution, every unrevealed move slot is a sample
        of it. Registered last (Adam state of the old weights kept). Later use: sampling search worlds from it."""
        if hasattr(self, "aux_bel"):
            return
        dev = self.aux_ko.weight.device
        self.bel_sizes = [self.e_species.num_embeddings, self.e_ability.num_embeddings, self.e_item.num_embeddings,
                          self.e_move.num_embeddings]
        self.aux_bel = nn.Linear(self.d, sum(self.bel_sizes)).to(dev)
        nn.init.zeros_(self.aux_bel.weight)      # uniform guesses at first (a random init x bel_scale gave huge losses and
        nn.init.zeros_(self.aux_bel.bias)        # gradients that crowded the policy's out of the clipped update)
        # output x this factor: with Adam the fresh head learns `factor` times faster than the rest (as jt_scale); a buffer,
        # so checkpoints carry it
        self.register_buffer("bel_scale", torch.tensor(1.0, device=dev))

    def add_unc_input(self):
        """(2026-10-20) Input of the uncertainty-aware damage block (obs_f[UNC0:UNC0+68], encode.rs encode_unc; zeros
        unless vgcsim.set_unc_library was called): per foe slot 34 floats (its threat on our two slots, our 8 moves' damage
        on it, speed order shares) added to that foe's Pokemon token, per move token of ours 6 floats (vs each foe slot).
        Zero-initialised: plays exactly as before at first; unc_scale = learning speed (as tok_hist_scale)."""
        if hasattr(self, "tok_unc_foe"):
            return
        dev = self.aux_ko.weight.device
        self.tok_unc_foe = nn.Linear(34, self.d).to(dev)
        self.tok_unc_mv = nn.Linear(6, self.d).to(dev)
        for l in (self.tok_unc_foe, self.tok_unc_mv):
            nn.init.zeros_(l.weight)
            nn.init.zeros_(l.bias)
        self.register_buffer("unc_scale", torch.tensor(1.0, device=dev))

    def add_ext_input(self):
        """(2026-10-22) Input of the extended damage block (obs_f[EXT0:EXT0+8*68], encode.rs encode_ext): per Pokemon
        token 68 floats = the bench block (a benched Pokemon's moves vs the other side's actives, theirs on it, speed
        order) + the same for its Mega form (stone holders while the side can still Mega). Zero-initialised: plays
        exactly as before at first; ext_scale = learning speed (as tok_hist_scale)."""
        if hasattr(self, "tok_ext"):
            return
        dev = self.aux_ko.weight.device
        self.tok_ext = nn.Linear(EXT_F, self.d).to(dev)
        nn.init.zeros_(self.tok_ext.weight)
        nn.init.zeros_(self.tok_ext.bias)
        self.register_buffer("ext_scale", torch.tensor(1.0, device=dev))

    def add_opp_cond(self):
        """(2026-11-05, v3) Opponent-conditioned policy: the opponent head's predicted distribution over each of their slots'
        38 actions (softmax, gradient stopped) -> one vector added to our Pokemon tokens and our move vectors before the
        policy heads. Conditions on the whole distribution, never one reply. Zero-initialised: plays exactly as before at
        first; oc_scale = learning speed. The opponent / auxiliary outputs still read the unconditioned encoder output."""
        if hasattr(self, "oc_proj"):
            return
        dev = self.aux_ko.weight.device
        self.oc_proj = nn.Linear(2 * 38, self.d).to(dev)
        nn.init.zeros_(self.oc_proj.weight)
        nn.init.zeros_(self.oc_proj.bias)
        self.register_buffer("oc_scale", torch.tensor(1.0, device=dev))

    def add_type_bias(self, dex_path=None):
        """(2026-11-05, v3, 13-token nets) Pairwise attention bias between the 12 Pokemon tokens from the type chart
        (best attacking type of A vs B's types and the reverse, in log2 steps) and speed order (base speed x stage x
        paralysis), per head through a zero-initialised Linear: attends exactly as before at first."""
        if hasattr(self, "tb_lin"):
            return
        dev = self.aux_ko.weight.device
        H = max(1, self.d // 64)
        self.tb_lin = nn.Linear(4, H).to(dev)
        nn.init.zeros_(self.tb_lin.weight)
        nn.init.zeros_(self.tb_lin.bias)
        self.register_buffer("tb_scale", torch.tensor(1.0, device=dev))
        chart = torch.zeros(20, 20)
        path = dex_path or paths.DEX
        try:
            with open(path, encoding="utf-8") as fh:
                types = json.load(fh)["types"]
            names = sorted(types)                      # the engine's type ids = sorted names (dex.rs)
            code = {1: 1.0, 2: -1.0, 3: -3.0}          # Showdown damageTaken: 1 weak, 2 resists, 3 immune
            for di, dn in enumerate(names):
                for an, c in types[dn].items():
                    if an in names and int(c) in code:
                        chart[names.index(an), di] = code[int(c)]
        except (OSError, KeyError, ValueError) as e:
            print(f"type bias: no type chart ({e}): the bias uses speed only", flush=True)
        self.register_buffer("tb_chart", chart.to(dev))

    def _type_bias(self, m12, ok12, trick_room=None):
        """[b, H, 13, 13] attention bias from the 12 Pokemon tokens' features (field token row / column 0)"""
        b = m12.shape[0]
        T = m12[..., 31:51]                                                  # [b, 12, 20] type one-hot
        M = T @ self.tb_chart.t().to(T.dtype)                                # [b, 12 def j, 20 att a]: sum over j's types
        # (2026-11-05, v3 review) any immune type makes the attack type immune (the sum gave immune + weak = -2)
        imm = (T @ (self.tb_chart == -3).t().to(T.dtype)) > 0.5
        M = torch.where(imm, torch.full_like(M, -3.0), M)
        eff = (M.unsqueeze(1) + (T.unsqueeze(2) - 1) * 1e4).amax(-1)       # [b, i, j] best type of i into j
        eff = torch.where(eff > -1e3, eff, torch.zeros_like(eff))
        st = (m12[..., 16] * 6).round()
        mult = torch.where(st >= 0, (2 + st) / 2, 2 / (2 - st))
        spe = (m12[..., 30] * 200).clamp(min=1) * mult * torch.where(m12[..., 7] > 0.5, 0.5, 1.0)
        ls = torch.log(spe)
        sp = torch.tanh((ls.unsqueeze(2) - ls.unsqueeze(1)) * 3)
        if trick_room is not None:
            sp = sp * torch.where(trick_room, -1.0, 1.0).to(sp.dtype).view(b, 1, 1)       # (2026-11-05) slower moves first
        feat = torch.stack([eff / 2, eff.transpose(1, 2) / 2, sp, torch.ones_like(sp)], -1)
        pair = (ok12.unsqueeze(2) & ok12.unsqueeze(1)).to(feat.dtype).unsqueeze(-1)
        sc = self.tb_scale
        bias = F.linear(feat * pair, self.tb_lin.weight * sc, self.tb_lin.bias * sc) * pair    # [b, 12, 12, H]
        return F.pad(bias.permute(0, 3, 1, 2), (0, 1, 0, 1)).float()

    def add_v3_heads(self):
        """(2026-11-05, v3) Auxiliary heads, training signal only (the reward is unchanged):
          aux_fate   per obs Pokemon token: alive at the game's end (logit), final HP fraction
          aux_margin field token: final margin (our final HP fractions summed minus theirs): mean, log variance
          aux_short  field token: the result discounted by gamma 0.9 / 0.99 per decision (tanh)
          aux_fnext  per token + aux_fnone (field): which Pokemon faints next (9-way: 8 tokens or none)
          aux_leave  per token: decisions until an active Pokemon leaves the field (0..5, 6+)
          aux_stat3  per token: status at our 3rd next decision (none, brn, par, psn, tox, slp, frz)
          aux_stage  per token: stat-stage change (7 stages, / 6) at the next decision
          aux_onext  their slot k: their action at our NEXT decision (38, their numbering)
          aux_mega   per foe token: Mega Evolves at our 1st / 2nd / 3rd / later next decision, or never (5)
          aux_pq / aux_pk / aux_pbias  pairwise damage, attacker token x target token (query . key / sqrt(32), 2 channels):
                     damage dealt by moves over the rest of the game / until the current turn ends, as a share of the
                     target's max HP (engine record: Battle.track)
          aux_order  per token: a score; P(A uses its move before B this turn) = sigmoid(score A - score B)
          aux_bspread per foe token (obs rows 4-7): stat points per stat as 4 bins (0 / 1-15 / 16-31 / 32), then the
                     nature's raised and lowered stat (Atk..Spe, neutral): 6 x 4 + 6 + 6 logits (extended belief)
          aux_brought per opponent roster token (v3 rows 2-7: unseen team members): brought (logit) (extended belief)
          aux_oq / aux_ok  1v1 matchups, our token i x their token j: tanh(query . key / 4) = the value of a 1v1 between the
                     two (everything else gone, field kept) from the solver's "average luck" game
        Zero-initialised outputs (aux_pk is a normal layer: with aux_pq at zero the product starts at zero); registered
        last (Adam state of the old weights kept). Missing heads are added to a net that has some of them."""
        d = self.d
        dev = self.aux_ko.weight.device
        for nm, i_, o_ in (("aux_fate", d, 2), ("aux_margin", d, 2), ("aux_short", d, 2), ("aux_fnext", d, 1),
                           ("aux_fnone", d, 1), ("aux_leave", d, 7), ("aux_stat3", d, 7), ("aux_stage", d, 7),
                           ("aux_onext", 2 * d, 38), ("aux_mega", d, 5), ("aux_pq", d, 64), ("aux_pk", d, 64),
                           ("aux_order", d, 1), ("aux_bspread", d, 36), ("aux_brought", d, 1), ("aux_oq", d, 16),
                           ("aux_ok", d, 16)):
            if hasattr(self, nm):
                continue
            lin = nn.Linear(i_, o_).to(dev)
            if nm not in ("aux_pk", "aux_ok"):
                nn.init.zeros_(lin.weight)
                nn.init.zeros_(lin.bias)
            setattr(self, nm, lin)
        if not hasattr(self, "aux_pbias"):
            self.aux_pbias = nn.Parameter(torch.zeros(2, device=dev))

    def add_belief_input(self):
        """(2026-11-05, v3 external belief input, 13-token nets) What we believe about each opponent row's set (obs_f
        [BEL0:OBS_BEL]: a library guess, an open team sheet, a known team, spread inference; in self-play a mix of exact /
        guessed / none): ability, item and mean move embeddings + flags and stats -> added to that opponent's token. Zero-
        initialised output (bel_scale like the other fresh layers); observations without the block add nothing."""
        if hasattr(self, "bel_in"):
            return
        d = self.d
        dev = self.aux_ko.weight.device
        self.bel_in = nn.Linear(9 + self.e_ability.embedding_dim + self.e_item.embedding_dim + self.e_move.embedding_dim, d).to(dev)
        self.bel_out = nn.Linear(d, d).to(dev)
        nn.init.zeros_(self.bel_out.weight)
        nn.init.zeros_(self.bel_out.bias)
        self.register_buffer("bel_in_scale", torch.tensor(1.0, device=dev))

    def _bel_tokens(self, obs_f, b):
        """[b, 10, d] belief embeddings (zeros without the layer / block)"""
        if not hasattr(self, "bel_in") or obs_f.shape[1] < OBS_BEL:
            return None
        B = obs_f[:, BEL0: OBS_BEL].reshape(b, 10, BEL_F)
        idx = B[:, :, 2:8].round().long().clamp(min=0)
        ab = self.e_ability(idx[:, :, 0].clamp(max=self.e_ability.num_embeddings - 1))
        it = self.e_item(idx[:, :, 1].clamp(max=self.e_item.num_embeddings - 1))
        mv_ids = idx[:, :, 2:6].clamp(max=self.e_move.num_embeddings - 1)
        mv = self.e_move(mv_ids)
        mok = (mv_ids > 0).unsqueeze(-1).to(mv.dtype)
        mv = (mv * mok).sum(2) / mok.sum(2).clamp(min=1)
        x = torch.cat([B[:, :, 0:2], B[:, :, 14:15], B[:, :, 8:14], ab, it, mv], -1)     # [14]: stats exact too
        pres = B[:, :, 0:1]
        sc = self.bel_in_scale
        out = F.linear(F.gelu(self.bel_in(x)), self.bel_out.weight * sc, self.bel_out.bias * sc)
        return out * pres

    def add_memory(self):
        """(2026-11-05, v3, 13-token nets) Game memory: the row's last 16 decisions of this game as compact frames (encoding.py
        mem_frame, obs_f[MEM0:MEM0 + MEM_W]) -> 16 memory tokens (frame projection + age embedding) and a learned null token;
        the 13 tokens cross-attend to them once, after the middle encoder layer. The output projection starts at zero: plays
        exactly as before at first. Observations without the memory block (e.g. 2308-wide data) give an empty memory."""
        if hasattr(self, "mem_in"):
            return
        d = self.d
        dev = self.aux_ko.weight.device
        self.mem_in = nn.Linear(MEM_F, d).to(dev)
        self.mem_age = nn.Embedding(MEM_K, d).to(dev)
        self.mem_null = nn.Parameter(torch.zeros(1, 1, d, device=dev))
        self.mem_norm = nn.LayerNorm(d).to(dev)
        self.mem_q = nn.Linear(d, d).to(dev)
        self.mem_kv = nn.Linear(d, 2 * d).to(dev)
        self.mem_out = nn.Linear(d, d).to(dev)
        nn.init.zeros_(self.mem_out.weight)
        nn.init.zeros_(self.mem_out.bias)

    def _mem_tokens(self, obs_f):
        """(keys [b, 17, d], valid [b, 17]) or None"""
        if not hasattr(self, "mem_in") or obs_f.shape[1] < MEM0 + MEM_W:
            return None
        b = obs_f.shape[0]
        mf = obs_f[:, MEM0: MEM0 + MEM_W].reshape(b, MEM_K, MEM_F)
        M = self.mem_in(mf) + self.mem_age.weight
        valid = mf[:, :, 0] > 0.5
        M = torch.cat([self.mem_null.expand(b, 1, self.d).to(M.dtype), M], 1)
        valid = torch.cat([torch.ones(b, 1, dtype=torch.bool, device=obs_f.device), valid], 1)
        return M, valid

    def _mem_attend(self, x, mem):
        M, valid = mem
        b, L, d = x.shape
        H = max(1, d // 64)
        hd = d // H
        q = self.mem_q(self.mem_norm(x)).view(b, L, H, hd).transpose(1, 2)
        kv = self.mem_kv(M).view(b, M.shape[1], 2, H, hd).permute(2, 0, 3, 1, 4)
        k, v = kv.unbind(0)
        sc = torch.matmul(q, k.transpose(-1, -2)).float() * (hd ** -0.5)
        sc = sc.masked_fill(~valid[:, None, None, :], float("-inf"))
        o = torch.matmul(torch.softmax(sc, -1).to(v.dtype), v).transpose(1, 2).reshape(b, L, d)
        return x + self.mem_out(o)

    def _v3_out(self, h, w_foe, roster_h=None):
        """the v3 auxiliary outputs (dict) from the 25-row layout"""
        b = h.shape[0]
        hm, hf = h[:, :8], h[:, -1]
        foe_slot = torch.einsum("bjk,bjd->bkd", w_foe, h[:, 4:8])           # [b, 2, d]
        return {"fate": self.aux_fate(hm), "margin": self.aux_margin(hf), "short": torch.tanh(self.aux_short(hf)),
                "fnext": torch.cat([self.aux_fnext(hm).squeeze(-1), self.aux_fnone(hf)], -1),
                "leave": self.aux_leave(hm), "stat3": self.aux_stat3(hm), "stage": self.aux_stage(hm),
                "onext": self.aux_onext(torch.cat([foe_slot, hf.unsqueeze(1).expand(b, 2, self.d)], -1)),
                "mega": self.aux_mega(h[:, 4:8]),
                "pdmg": (torch.einsum("bick,bjck->bcij", self.aux_pq(hm).view(b, 8, 2, 32), self.aux_pk(hm).view(b, 8, 2, 32))
                         * (32 ** -0.5) + self.aux_pbias.view(1, 2, 1, 1)),
                "order": self.aux_order(hm).squeeze(-1),
                "bspread": self.aux_bspread(h[:, 4:8]),
                "ovo": torch.tanh(torch.einsum("bik,bjk->bij", self.aux_oq(h[:, :4]), self.aux_ok(h[:, 4:8])) * 0.25),
                "brought": (self.aux_brought(roster_h[:b]).squeeze(-1) if roster_h is not None and roster_h.shape[0] >= b
                            else h.new_zeros(b, 6))}

    def _unc(self, obs_f, b):
        """(per foe slot [b, 2, d], per our move token [b, 8, d]) from the uncertainty block"""
        U = obs_f[:, UNC0: UNC0 + UNC_F]
        thr = U[:, :16].view(b, 2, 8)                                       # [b, j, (k, c)]
        our = U[:, 16:64].view(b, 2, 4, 2, 3)                               # [b, k, m, j, c]
        spd = U[:, 64:68].view(b, 2, 2)                                     # [b, k, j]
        foe = torch.cat([thr, our.permute(0, 3, 1, 2, 4).reshape(b, 2, 24), spd.transpose(1, 2)], -1)
        sc = self.unc_scale
        return (F.linear(foe, self.tok_unc_foe.weight * sc, self.tok_unc_foe.bias * sc),
                F.linear(our.reshape(b, 8, 6), self.tok_unc_mv.weight * sc, self.tok_unc_mv.bias * sc))

    def _joint(self, h, w_our, w_foe, b):
        """the interaction I [b, 38, 38] (slot 1 action x slot 2 action)"""
        d = self.d
        our_h, foe_h = h[:, :4], h[:, 4:8]
        slot_mon = torch.einsum("bik,bid->bkd", w_our, our_h)               # [b, 2, d]
        foe_slot = torch.einsum("bjk,bjd->bkd", w_foe, foe_h)
        W = self.jt_mlp.weight
        mv = F.linear(h[:, 8:16], W).view(b, 2, 4, -1)                       # [b, 2, 4 moves, w]
        fo = F.linear(foe_slot, W)
        sm = F.linear(slot_mon, W, self.jt_mlp.bias)                         # (bias once, with the slot term)
        w_ = mv.shape[-1]
        tg = torch.stack([F.linear(self.no_target, W).expand(b, 2, w_), fo[:, :1].expand(b, 2, w_), fo[:, 1:].expand(b, 2, w_),
                          F.linear(slot_mon, W).flip(1)], 2)                 # [b, 2, 4 targets, w]
        xm = (mv[:, :, :, None, None] + tg[:, :, None, :, None] + self.jt_mega[None, None, None, None]).reshape(b, 2, 32, w_)
        xs = (F.linear(our_h, W) + self.jt_sw).unsqueeze(1).expand(b, 2, 4, w_)
        xo = self.jt_other.expand(b, 2, 2, w_)
        z = torch.relu(torch.cat([xm, xs, xo], 2) + sm.unsqueeze(2))         # [b, 2, 38, w]
        A, B = self.jt_a(z) * self.jt_scale, self.jt_b(z) * self.jt_scale   # [b, 2, 38, r]
        return torch.einsum("bir,bjr->bij", A[:, 0], B[:, 1]) + torch.einsum("bir,bjr->bij", B[:, 0], A[:, 1])

    def _opp_joint(self, h, w_our, w_foe, ids, idx_foe, ar, b):
        """the opponent's slot interaction I [b, 38, 38] (their slot 1 action x their slot 2 action, their numbering)"""
        d = self.d
        our_h, foe_h = h[:, :4], h[:, 4:8]
        slot_mon = torch.einsum("bik,bid->bkd", w_our, our_h)               # [b, 2, d]
        foe_slot = torch.einsum("bjk,bjd->bkd", w_foe, foe_h)
        if self.tokens in (25, 13):
            omv = h[:, 16:24].view(b, 2, 4, d)
        else:
            fmv_ids = torch.stack([ids[ar, idx_foe[:, j], 3:7] for j in range(2)], 1)
            omv = self.opp_mv(self.e_move(fmv_ids)) + foe_slot.unsqueeze(2)
        W = self.oj_mlp.weight
        mv = F.linear(omv, W)                                                # [b, 2, 4 moves, w]
        us = F.linear(slot_mon, W)                                           # targets: our slot a / b
        sm = F.linear(foe_slot, W, self.oj_mlp.bias)                         # (bias once, with the slot term)
        w_ = mv.shape[-1]
        tg = torch.stack([F.linear(self.no_target, W).expand(b, 2, w_), us[:, :1].expand(b, 2, w_), us[:, 1:].expand(b, 2, w_),
                          F.linear(foe_slot, W).flip(1)], 2)                 # [b, 2, 4 targets, w]: none / our a / our b / partner
        xm = (mv[:, :, :, None, None] + tg[:, :, None, :, None] + self.oj_mega[None, None, None, None]).reshape(b, 2, 32, w_)
        xs = (F.linear(foe_h, W) + self.oj_sw).unsqueeze(1).expand(b, 2, 4, w_)
        xo = self.oj_other.expand(b, 2, 2, w_)
        z = torch.relu(torch.cat([xm, xs, xo], 2) + sm.unsqueeze(2))         # [b, 2, 38, w]
        A, B = self.oj_a(z) * self.oj_scale, self.oj_b(z) * self.oj_scale
        return torch.einsum("bir,bjr->bij", A[:, 0], B[:, 1]) + torch.einsum("bir,bjr->bij", B[:, 0], A[:, 1])

    def forward(self, obs_f, obs_i, opp=False, aux=False, fut=False, joint=False, belief=False, opp_joint=False, aux_n=None,
                vdist=False, v3aux=False):
        """aux_n (2026-10-23): the auxiliary outputs (opponent, opponent joint, KO, future, belief) only for the
        FIRST aux_n rows (shorter tensors); policy, value and the joint table stay on every row.
        belief=True (needs add_belief_head): the belief logits [b, 4 foes, species + ability + item + move vocab] come
        right before the joint output (last when joint is off).
        joint=True (needs add_joint_head): the slot interaction I [b, 38, 38] is appended as the LAST output.
        opp_joint=True (needs add_opp_joint): the opponent's slot interaction [b, 38, 38] (their numbering) comes right
        before the joint output (last when joint is off).
        fut=True (needs add_future_heads): after the other outputs (None placeholders for opp / aux when off) also
        (hp [b, 8], surv [b, 8, 5], len [b, 9], wx [b, 10], spd [b, 3]) logits / predictions.
        aux=True: also the KO prediction (logits [b, 8]: will each of the 8 Pokemon faint before our next decision),
        returned as a 4th output (the 3rd is the opponent prediction, or None without opp).
        (2026-11-05) vdist=True (HL-Gauss nets): the value output is the [b, bins] logits instead of the mean.
        v3aux=True (needs add_v3_heads): a dict of the v3 auxiliary outputs right after the future outputs (before belief /
        opponent joint / joint, so their positions from the end don't change)."""
        b = obs_f.shape[0]
        mf = obs_f[:, : N_MON * MON_F].view(b, N_MON, MON_F)
        ids = obs_i.view(b, N_MON, MON_I).long()
        g = obs_f[:, N_MON * MON_F: N_MON * MON_F + FIELD_F]
        dm = obs_f[:, DMG0: DMG0 + 96]
        present = mf[:, :, 0] > 0.5
        ar = torch.arange(b, device=obs_f.device)
        # the Pokemon in each active slot (ours: k = 0, 1; theirs: j = 0, 1)
        w_our = mf[:, :4, 4:6]                                              # [b, 4, 2]
        w_foe = mf[:, 4:, 4:6]
        idx_our = w_our.argmax(1)                                           # [b, 2]
        idx_foe = w_foe.argmax(1) + 4
        has_our = w_our.sum(1) > 0
        has_foe = w_foe.sum(1) > 0
        extra = None
        if self.extra:
            extra = mf.new_zeros(b, N_MON, self.extra)
            dt = dm[:, 32:64].view(b, 2, 16)
            for j in range(2):
                extra[ar, idx_foe[:, j]] += dt[:, j] * has_foe[:, j:j + 1]
        mons = self.tok_mon(self._mon_x(mf, ids, extra))                  # [b, 8, d]
        if self.history and obs_f.shape[1] >= OBS_T + N_MON * HIST_F:
            # (observations without the history block, e.g. old search data: no history term)
            # input and weight zero-padded 20 -> 32 columns: same result, but an aligned inner size (K=20 sent the GEMMs
            # to slow unaligned kernels: the update took ~40% longer)
            h = F.pad(obs_f[:, OBS_T: OBS_T + N_MON * HIST_F].reshape(b * N_MON, HIST_F), (0, 32 - HIST_F))
            sc = self.tok_hist_scale
            mons = mons + F.linear(h, F.pad(self.tok_hist.weight * sc, (0, 32 - HIST_F)), self.tok_hist.bias * sc).view(b, N_MON, -1)
        # our active moves: [b, 2, 4, ...]
        mv_ids = torch.stack([ids[ar, idx_our[:, k], 3:7] for k in range(2)], 1)
        pp = torch.stack([mf[ar, idx_our[:, k], 53:57] for k in range(2)], 1)
        d_our = dm[:, :32].view(b, 2, 4, 4)
        d_ally = dm[:, 76:92].view(b, 2, 4, 2)
        my_mv = self.tok_my_move(torch.cat([self.e_move(mv_ids), d_our, d_ally, pp.unsqueeze(-1)], -1)).view(b, 8, -1)
        my_mv_ok = (has_our.unsqueeze(-1) & (mv_ids > 0)).view(b, 8)
        if hasattr(self, "tok_ext") and obs_f.shape[1] >= EXT0 + N_MON * EXT_F:
            x_ = obs_f[:, EXT0: EXT0 + N_MON * EXT_F].reshape(b * N_MON, EXT_F)
            sc = self.ext_scale
            mons = mons + F.linear(x_, self.tok_ext.weight * sc, self.tok_ext.bias * sc).view(b, N_MON, -1)
        if hasattr(self, "tok_unc_foe") and obs_f.shape[1] >= UNC0 + UNC_F:
            u_foe, u_mv = self._unc(obs_f, b)
            my_mv = my_mv + u_mv
            add = torch.zeros_like(mons)
            for j in range(2):
                add[ar, idx_foe[:, j]] += u_foe[:, j] * has_foe[:, j:j + 1].to(u_foe.dtype)
            mons = mons + add
        field = self.tok_field(torch.cat([g, dm[:, 64:76], dm[:, 92:96]], -1)).unsqueeze(1)
        one = torch.ones(b, 1, dtype=torch.bool, device=obs_f.device)
        if self.tokens == 13:
            h, value, roster_h = self._enc13(obs_f, mf, mons, present, my_mv, my_mv_ok, field[:, 0], w_our, w_foe, has_foe, ids,
                                   idx_foe, ar, dm, b, need_opp=bool(opp or opp_joint or v3aux or hasattr(self, "oc_proj")))
        elif self.tokens == 25:
            # their active moves
            fmv_ids = torch.stack([ids[ar, idx_foe[:, j], 3:7] for j in range(2)], 1)
            d_their = dm[:, 32:64].view(b, 2, 4, 4)
            foe_mv = self.tok_foe_move(torch.cat([self.e_move(fmv_ids), d_their], -1)).view(b, 8, -1)
            foe_mv_ok = has_foe.unsqueeze(-1).expand(b, 2, 4).reshape(b, 8)
            x = torch.cat([mons, my_mv, foe_mv, field], 1)
            keep = torch.cat([present, my_mv_ok, foe_mv_ok, one], 1)
        else:
            x = torch.cat([mons, my_mv, field], 1)
            keep = torch.cat([present, my_mv_ok, one], 1)
        if self.tokens != 13:
            x = x + self.kind(self.kinds) + self.pos.weight
            h = self.norm(self._encode(x, keep))
            hk = h * keep.unsqueeze(-1)
            pooled = hk.sum(1) / keep.sum(1, keepdim=True).clamp(min=1)
            value = self.value(torch.cat([h[:, -1], pooled], -1))
        if self.vbins:
            value = value if vdist else hlg_mean(value, self.v_centers)
        else:
            value = value.squeeze(-1)
        cap = (lambda x: self.softcap_v * torch.tanh(x / self.softcap_v)) if self.softcap else (lambda x: x)
        oc = hasattr(self, "oc_proj")
        h_pol = h
        ol_full = None
        if oc:
            # (v3) opponent-conditioned policy: the opponent head's distribution (gradient stopped) shifts our tokens
            ol_full = cap(self._heads(h, w_our, w_foe, ids, idx_foe, ar, b, True, opp_only=True)[1])
            # (2026-11-05, v3 review) switches to a foe row the view shows fainted or on the field are impossible
            fr_ = mf[:, 4:8]
            sw_bad = (fr_[..., 0] > 0.5) & ((fr_[..., 1] < 0.5) | (fr_[..., 3] > 0.5))          # [b, 4]
            olm = ol_full.detach().float().clone()
            olm[:, :, 32:36] = olm[:, :, 32:36].masked_fill(sw_bad.unsqueeze(1), -1e4)
            pr = torch.softmax(olm, -1).flatten(1).to(h.dtype)
            sc = self.oc_scale
            c_ = F.linear(pr, self.oc_proj.weight.to(h.dtype) * sc, self.oc_proj.bias.to(h.dtype) * sc).unsqueeze(1)
            h_pol = torch.cat([h[:, :4] + c_, h[:, 4:8], h[:, 8:16] + c_, h[:, 16:]], 1)
        if aux_n is not None and aux_n < b:
            logits, _ = self._heads(h_pol, w_our, w_foe, ids, idx_foe, ar, b, False)
            n_ = aux_n
            h_a, w_our_a, w_foe_a, ids_a, idx_foe_a, ar_a, b_a = h[:n_], w_our[:n_], w_foe[:n_], ids[:n_], idx_foe[:n_], ar[:n_], n_
            if opp:
                ol = ol_full[:n_] if ol_full is not None else cap(
                    self._heads(h_a, w_our_a, w_foe_a, ids_a, idx_foe_a, ar_a, b_a, True, opp_only=True)[1])
            else:
                ol = None
        else:
            if oc:
                logits = self._heads(h_pol, w_our, w_foe, ids, idx_foe, ar, b, False)[0]
                ol = ol_full if opp else None
            else:
                logits, ol = self._heads(h, w_our, w_foe, ids, idx_foe, ar, b, opp)
                ol = cap(ol) if ol is not None else None
            h_a, w_our_a, w_foe_a, ids_a, idx_foe_a, ar_a, b_a = h, w_our, w_foe, ids, idx_foe, ar, b
        logits = cap(logits)
        out = (logits, value, ol) if opp else (logits, value)
        if aux or fut:
            ko = self.aux_ko(h_a[:, :8]).squeeze(-1) if aux else None                  # [b, 8]
            out = out + ((None,) if not opp else ()) + (ko,)
        if fut:
            hm, hf = h_a[:, :8], h_a[:, -1]
            spd = self.aux_fspd(hf) if hasattr(self, "aux_fspd") else hf.new_zeros(hf.shape[0], 3)
            out = out + (self.aux_fhp(hm).squeeze(-1), self.aux_fsurv(hm), self.aux_flen(hf), self.aux_fwx(hf), spd)
        if v3aux:
            out = out + (self._v3_out(h_a, w_foe_a, roster_h if self.tokens == 13 else None),)
        if belief:
            out = out + (self.aux_bel(h_a[:, 4:8]) * self.bel_scale,)
        if opp_joint:
            out = out + (self._opp_joint(h_a, w_our_a, w_foe_a, ids_a, idx_foe_a, ar_a, b_a).float(),)
        if joint:
            out = out + (self._joint(h_pol, w_our, w_foe, b).float(),)
        return out

    @staticmethod
    def _qmask(ok):
        """decoder query padding mask; a row with no valid query keeps all of them (its outputs are never used), since
        attention over an all-masked row gives NaN"""
        return ~(ok | ~ok.any(1, keepdim=True))

    def _enc13(self, obs_f, mf, mons, present, my_mv, my_mv_ok, field, w_our, w_foe, has_foe, ids, idx_foe, ar, dm, b,
               need_opp):
        """13-token encoder + move decoders -> (h [b, 25, d] in the 25-token layout the heads read, value)"""
        d = self.d
        dev = obs_f.device
        if obs_f.shape[1] >= OBS_V3:
            v3 = obs_f[:, V3_0: OBS_V3].reshape(b, 8, V3_F)
            vmf, vids = v3[:, :, :MON_F], v3[:, :, MON_F:].round().long()
        else:                                            # observations without the V3 rows: no extra Pokemon
            vmf, vids = mf.new_zeros(b, 8, MON_F), ids.new_zeros(b, 8, MON_I)
        vt = self.tok_mon(self._mon_x(vmf, vids))                           # [b, 8, d]
        vpres = vmf[:, :, 0] > 0.5
        ours = torch.cat([mons[:, :4], vt[:, :2]], 1)
        ours_ok = torch.cat([present[:, :4], vpres[:, :2]], 1)
        # theirs: 10 candidates (obs rows 4-7, V3 rows 2-7) -> the first 6 present ones, stable order
        cand = torch.cat([mons[:, 4:], vt[:, 2:]], 1)
        bt = self._bel_tokens(obs_f, b)
        if bt is not None:
            cand = cand + bt.to(cand.dtype)
        cand_ok = torch.cat([present[:, 4:], vpres[:, 2:]], 1)
        order = torch.sort((~cand_ok).to(torch.int8), dim=1, stable=True).indices
        sel = order[:, :6]
        theirs = torch.gather(cand, 1, sel.unsqueeze(-1).expand(b, 6, d))
        theirs_ok = torch.gather(cand_ok, 1, sel)
        if obs_f.shape[1] >= OBS_FLAG:
            fl = obs_f[:, FLAG0: OBS_FLAG].reshape(b, 16, FLAG_F)
            ours = ours + self.tok_flag(torch.cat([fl[:, :4], fl[:, 8:10]], 1))
            cfl = torch.cat([fl[:, 4:8], fl[:, 10:16]], 1)
            theirs = theirs + self.tok_flag(torch.gather(cfl, 1, sel.unsqueeze(-1).expand(b, 6, FLAG_F)))
        kinds = torch.cat([self._kinds13.to(dev).expand(b, 6),
                           torch.where(sel < 4, 1, 3), torch.full((b, 1), 4, device=dev, dtype=torch.long)], 1)
        x = torch.cat([ours, theirs, field.unsqueeze(1)], 1) + self.kind(kinds) + self.pos.weight
        keep = torch.cat([ours_ok, theirs_ok, torch.ones(b, 1, dtype=torch.bool, device=dev)], 1)
        abias = None
        if hasattr(self, "tb_lin"):
            cmf = torch.cat([mf[:, 4:], vmf[:, 2:]], 1)
            m12 = torch.cat([mf[:, :4], vmf[:, :2], torch.gather(cmf, 1, sel.unsqueeze(-1).expand(b, 6, MON_F))], 1)
            abias = self._type_bias(m12.float(), keep[:, :12], obs_f[:, 8 * MON_F + 12] > 0)
        h = self.norm(self._encode(x, keep, abias, self._mem_tokens(obs_f)))   # [b, 13, d]
        hk = h * keep.unsqueeze(-1)
        pooled = hk.sum(1) / keep.sum(1, keepdim=True).clamp(min=1)
        hf = h[:, 12]
        value = self.value(torch.cat([hf, pooled], -1))
        # obs foe rows 4-7 back from their sorted positions (a row without a token: unseen_foe + field)
        rank = torch.argsort(order, dim=1)[:, :4]
        okr = present[:, 4:] & (rank < 6)
        foe_h = torch.gather(h[:, 6:12], 1, rank.clamp(max=5).unsqueeze(-1).expand(b, 4, d))
        foe_h = torch.where(okr.unsqueeze(-1), foe_h, (self.unseen_foe + hf).unsqueeze(1))
        roster_h = None
        if hasattr(self, "aux_brought"):
            # (2026-11-05, extended belief) the opponent's roster rows (v3 rows 2-7) back from their sorted positions, for
            # _v3_out's 'brought' head (returned, not kept on the module: compiled / graphed forwards stay side-effect free)
            rrank = torch.argsort(order, dim=1)[:, 4:10]
            rok = cand_ok[:, 4:] & (rrank < 6)
            rh = torch.gather(h[:, 6:12], 1, rrank.clamp(max=5).unsqueeze(-1).expand(b, 6, d))
            roster_h = torch.where(rok.unsqueeze(-1), rh, hf.unsqueeze(1))
        our_h = h[:, :4]
        mem_mask = ~keep
        slot_mon = torch.einsum("bik,bid->bkd", w_our, our_h)             # [b, 2, d]
        foe_slot = torch.einsum("bjk,bjd->bkd", w_foe, foe_h)
        q = my_mv + self.dec_pos.weight + slot_mon.unsqueeze(2).expand(b, 2, 4, d).reshape(b, 8, d)
        my_dec = self.dec_norm(self.dec(q, h, tgt_key_padding_mask=self._qmask(my_mv_ok), memory_key_padding_mask=mem_mask))
        if need_opp:
            fmv_ids = torch.stack([ids[ar, idx_foe[:, j], 3:7] for j in range(2)], 1)     # [b, 2, 4]
            oq = (self.opp_mv(self.e_move(fmv_ids)) + self.odec_dmg(dm[:, 32:64].reshape(b, 2, 4, 4))
                  + foe_slot.unsqueeze(2)).reshape(b, 8, d) + self.odec_pos.weight
            ook = (has_foe.unsqueeze(-1) & (fmv_ids > 0)).reshape(b, 8)
            op_dec = self.odec_norm(self.odec(oq, h, tgt_key_padding_mask=self._qmask(ook), memory_key_padding_mask=mem_mask))
        else:
            op_dec = h.new_zeros(b, 8, d)
        return torch.cat([our_h, foe_h, my_dec, op_dec, hf.unsqueeze(1)], 1), value, roster_h

    def _lin(self, lay, name, x):
        """one encoder Linear: the module's own maths, or (fp8 inference) the fp8 weights"""
        a = lay.self_attn
        w, bias = {"qkv": (a.in_proj_weight, a.in_proj_bias), "out": (a.out_proj.weight, a.out_proj.bias),
                   "l1": (lay.linear1.weight, lay.linear1.bias), "l2": (lay.linear2.weight, lay.linear2.bias)}[name]
        if getattr(self, "_fp8", False) and not torch.is_grad_enabled():
            return fp8_linear(x, getattr(lay, f"_w8_{name}"), getattr(lay, f"_s8_{name}"), bias)
        return F.linear(x, w, bias)

    def _encode(self, x, keep, abias=None, mem=None):
        """The encoder layers. attn_impl "manual" (2026-10-09) runs the same layers (same weights, pre-norm, ReLU, no dropout)
        with attention written out: with 17 tokens the fused memory-efficient attention kernel works on 64 x 64 tiles that
        are mostly padding (~13% of the GPU time in the real run's profile); written out, the compiler fuses it instead.
        (2026-11-05) v3 encoder options (enc_opts) and the type / speed attention bias (abias [b, H, L, L]) need the
        written-out path, whatever attn_impl says."""
        special = bool(self.enc_opts) or abias is not None or mem is not None
        if not special and self.attn_impl not in ("manual", "triton", "fused") and not (getattr(self, "_fp8", False) and not torch.is_grad_enabled()):
            return self.enc(x, src_key_padding_mask=~keep)
        b, L, d = x.shape
        if special:
            return self._encode_v3(x, keep, abias, mem)
        if self.attn_impl == "triton" and x.is_cuda and not torch.is_grad_enabled():
            import tri_attn            # (2026-10-18) fused inference kernel, rental only (see tri_attn.py); else manual
            if tri_attn.triton is not None and tri_attn.STATE["ok"] is not False:
                for lay in self.enc.layers:
                    a = lay.self_attn
                    H = a.num_heads
                    hd = d // H
                    qkv = self._lin(lay, "qkv", lay.norm1(x)).view(b, L, 3, H, hd).permute(2, 0, 3, 1, 4)
                    o = tri_attn.attn(qkv[0], qkv[1], qkv[2], keep, hd ** -0.5)
                    x = x + self._lin(lay, "out", o.transpose(1, 2).reshape(b, L, d))
                    x = x + self._lin(lay, "l2", lay.activation(self._lin(lay, "l1", lay.norm2(x))))
                return x
        bias = torch.zeros(b, 1, 1, L, device=x.device, dtype=torch.float64 if x.dtype == torch.float64 else torch.float32).masked_fill(~keep[:, None, None, :], float("-inf"))
        if self.attn_impl == "fused":
            # (2026-10-23, rental test) the same layers with F.scaled_dot_product_attention on an explicit additive mask;
            # TNET_SDPA_BACKEND = flash | cudnn | efficient | math forces one kernel (unset = PyTorch's choice). A backend that
            # can't run this call (e.g. flash with a mask) raises at the first call: the test then reports it as unusable.
            ctx = _sdpa_ctx()
            with ctx:
                for lay in self.enc.layers:
                    a = lay.self_attn
                    H = a.num_heads
                    hd = d // H
                    qkv = self._lin(lay, "qkv", lay.norm1(x)).view(b, L, 3, H, hd).permute(2, 0, 3, 1, 4)
                    q, k, v = qkv.unbind(0)
                    o = F.scaled_dot_product_attention(q, k, v, attn_mask=bias.to(q.dtype))
                    x = x + self._lin(lay, "out", o.transpose(1, 2).reshape(b, L, d))
                    x = x + self._lin(lay, "l2", lay.activation(self._lin(lay, "l1", lay.norm2(x))))
            return x
        for lay in self.enc.layers:
            a = lay.self_attn
            H = a.num_heads
            hd = d // H
            qkv = self._lin(lay, "qkv", lay.norm1(x)).view(b, L, 3, H, hd).permute(2, 0, 3, 1, 4)
            q, k, v = qkv.unbind(0)                                  # [b, H, L, hd]
            sc = torch.matmul(q, k.transpose(-1, -2))
            sc = (sc.float() if sc.dtype in (torch.float16, torch.bfloat16) else sc) * (hd ** -0.5) + bias
            o = torch.matmul(torch.softmax(sc, -1).to(v.dtype), v)          # [b, H, L, hd]
            x = x + self._lin(lay, "out", o.transpose(1, 2).reshape(b, L, d))
            x = x + self._lin(lay, "l2", lay.activation(self._lin(lay, "l1", lay.norm2(x))))
        return x

    def _encode_v3(self, x, keep, abias, mem=None):
        """the written-out encoder with the v3 options: QK-norm (RMS-norm of q and k per head, learned gain), gated
        attention (sigmoid(Linear(normed input)) per head on the attention output), value residual (v = v + a_l (v_0 - v)),
        the additive attention bias; zero-init output projections are an initialisation only"""
        b, L, d = x.shape
        opts = self.enc_opts
        bias = torch.zeros(b, 1, 1, L, device=x.device, dtype=torch.float32).masked_fill(~keep[:, None, None, :], float("-inf"))
        if abias is not None:
            bias = bias + abias
        v0 = None
        for li, lay in enumerate(self.enc.layers):
            a = lay.self_attn
            H = a.num_heads
            hd = d // H
            xn = lay.norm1(x)
            qkv = self._lin(lay, "qkv", xn).view(b, L, 3, H, hd).permute(2, 0, 3, 1, 4)
            q, k, v = qkv.unbind(0)                                  # [b, H, L, hd]
            if opts & 1:
                q = F.rms_norm(q, (hd,)) * self.qk_g[li, 0].to(q.dtype)
                k = F.rms_norm(k, (hd,)) * self.qk_g[li, 1].to(k.dtype)
            if opts & 4:
                if li == 0:
                    v0 = v
                else:
                    v = v + self.vres_a[li].to(v.dtype) * (v0 - v)
            sc = torch.matmul(q, k.transpose(-1, -2))
            sc = (sc.float() if sc.dtype in (torch.float16, torch.bfloat16) else sc) * (hd ** -0.5) + bias
            o = torch.matmul(torch.softmax(sc, -1).to(v.dtype), v)          # [b, H, L, hd]
            if opts & 2:
                o = o * torch.sigmoid(self.attn_gate[li](xn)).to(o.dtype).transpose(1, 2).unsqueeze(-1)
            x = x + self._lin(lay, "out", o.transpose(1, 2).reshape(b, L, d))
            x = x + self._lin(lay, "l2", lay.activation(self._lin(lay, "l1", lay.norm2(x))))
            if mem is not None and li == (len(self.enc.layers) - 1) // 2:
                x = self._mem_attend(x, mem)
        return x

    def _heads(self, h, w_our, w_foe, ids, idx_foe, ar, b, opp, opp_only=False):
        """Policy (and opponent) logits. Each head's first Linear reads cat[move, target, self]; it is applied per input
        block and the blocks added (Linear(cat[x, y, z]) = Wx x + Wy y + Wz z + bias: the same function, 2026-10-09), so
        the [b, 4 moves, 4 targets, 3d] pair tensor is never built (~16x fewer multiplications in that layer)."""
        d = self.d
        our_h, foe_h = h[:, :4], h[:, 4:8]
        slot_mon = torch.einsum("bik,bid->bkd", w_our, our_h)               # [b, 2 slots, d]
        foe_slot = torch.einsum("bjk,bjd->bkd", w_foe, foe_h)

        def pairs(lin, mv, me, other, ally):
            # mv [b, 2, 4, d] move vectors; targets per slot k: none / other[0] / other[1] / ally[1 - k]; self = me[k]
            W, bias = lin[0].weight, lin[0].bias
            Wm, Wt, Ws = W[:, :d], W[:, d:2 * d], W[:, 2 * d:]
            t_none = F.linear(self.no_target, Wt).expand(b, 2, d)
            t_oth = F.linear(other, Wt)
            tg = torch.stack([t_none, t_oth[:, :1].expand(b, 2, d), t_oth[:, 1:].expand(b, 2, d),
                              F.linear(ally, Wt).flip(1)], 2)                  # [b, 2, 4 targets, d]
            pre = F.linear(mv, Wm).unsqueeze(3) + tg.unsqueeze(2) + F.linear(me, Ws, bias)[:, :, None, None, :]
            return lin[2](lin[1](pre)).reshape(b, 2, 32)                         # (move, target, mega)

        def switches(lin, me, team):
            W, bias = lin[0].weight, lin[0].bias
            x = F.linear(me, W[:, :d], bias).unsqueeze(2) + F.linear(team, W[:, d:]).unsqueeze(1)   # [b, 2, 4, d]
            return lin[2](lin[1](x)).squeeze(-1)

        if opp_only:
            logits = None
        else:
            lm = pairs(self.pol_move, h[:, 8:16].view(b, 2, 4, d), slot_mon, foe_slot, slot_mon)
            logits = torch.cat([lm, switches(self.pol_switch, slot_mon, our_h), self.pol_other(slot_mon)], -1)
        if not opp:
            return logits, None
        # their move m from their slot j into (none / our slot a / our slot b / their partner), +-Mega
        if self.tokens in (25, 13):
            omv = h[:, 16:24].view(b, 2, 4, d)
        else:
            fmv_ids = torch.stack([ids[ar, idx_foe[:, j], 3:7] for j in range(2)], 1)
            omv = self.opp_mv(self.e_move(fmv_ids)) + foe_slot.unsqueeze(2)
        olm = pairs(self.opp_move, omv, foe_slot, slot_mon, foe_slot)
        ol = torch.cat([olm, switches(self.opp_switch, foe_slot, foe_h), self.opp_other(foe_slot)], -1)
        return logits, ol

    def _heads_ref(self, h, w_our, w_foe, ids, idx_foe, ar, b, opp):
        """The original (pair tensor) heads: kept as the reference implementation"""
        our_h, foe_h = h[:, :4], h[:, 4:8]
        slot_mon = [(w_our[:, :, k:k + 1] * our_h).sum(1) for k in range(2)]        # [b, d]
        foe_slot = [(w_foe[:, :, j:j + 1] * foe_h).sum(1) for j in range(2)]
        logits = []
        for k in range(2):
            mv = h[:, 8 + 4 * k: 12 + 4 * k]                                        # [b, 4, d]
            tg = torch.stack([self.no_target.expand(b, -1), foe_slot[0], foe_slot[1], slot_mon[1 - k]], 1)   # [b, 4 targets, d]
            pair = torch.cat([mv.unsqueeze(2).expand(b, 4, 4, self.d), tg.unsqueeze(1).expand(b, 4, 4, self.d),
                              slot_mon[k][:, None, None, :].expand(b, 4, 4, self.d)], -1)
            lm = self.pol_move(pair).reshape(b, 32)                                  # (move, target, mega)
            ls = self.pol_switch(torch.cat([slot_mon[k].unsqueeze(1).expand(b, 4, self.d), our_h], -1)).squeeze(-1)
            lo = self.pol_other(slot_mon[k])
            logits.append(torch.cat([lm, ls, lo], -1))
        if opp:
            # their move m from their slot j into (none / our slot a / our slot b / their partner), +-Mega
            fmv_ids = torch.stack([ids[ar, idx_foe[:, j], 3:7] for j in range(2)], 1)
            ol = []
            for j in range(2):
                if self.tokens == 25:
                    mv = h[:, 16 + 4 * j: 20 + 4 * j]
                else:
                    mv = self.opp_mv(self.e_move(fmv_ids[:, j])) + foe_slot[j].unsqueeze(1)
                tg = torch.stack([self.no_target.expand(b, -1), slot_mon[0], slot_mon[1], foe_slot[1 - j]], 1)
                pair = torch.cat([mv.unsqueeze(2).expand(b, 4, 4, self.d), tg.unsqueeze(1).expand(b, 4, 4, self.d),
                                  foe_slot[j][:, None, None, :].expand(b, 4, 4, self.d)], -1)
                lm = self.opp_move(pair).reshape(b, 32)
                ls = self.opp_switch(torch.cat([foe_slot[j].unsqueeze(1).expand(b, 4, self.d), foe_h], -1)).squeeze(-1)
                ol.append(torch.cat([lm, ls, self.opp_other(foe_slot[j])], -1))
            return torch.stack(logits, 1), torch.stack(ol, 1)
        return torch.stack(logits, 1), None

    def preview(self, pf, pi):
        b = pf.shape[0]
        mf = pf.view(b, 12, MON_F)
        ids = pi.view(b, 12, MON_I).long()
        mons = self.tok_mon(self._mon_x(mf, ids))
        side = torch.cat([torch.zeros(6, dtype=torch.long), torch.ones(6, dtype=torch.long)]).to(pf.device)
        if self.tokens == 13:
            # (2026-11-05, v3) the preview runs through the battle's 13-token layout: our 6 / their 6 at the battle's
            # positions, an empty field token (kind 4) in the summary slot
            fld = self.tok_field(pf.new_zeros(1, FIELD_F + 16)) + self.kind.weight[4]
            x = torch.cat([mons + self.kind(side), fld.expand(b, 1, self.d)], 1) + self.pos.weight
        else:
            x = torch.cat([mons + self.kind(side), self.pv_cls.expand(b, 1, self.d)], 1) + self.pv_pos.weight
        present = mf[:, :, 0] > 0.5
        keep = torch.cat([present, torch.ones(b, 1, dtype=torch.bool, device=pf.device)], 1)
        h = self.norm(self._encode(x, keep))
        own, cls = h[:, :6] * present[:, :6].unsqueeze(-1), h[:, 12]
        z = torch.relu(torch.einsum("ok,bkd->bod", self.pv_w4, self.pv_p4(own))
                       + torch.einsum("ok,bkd->bod", self.pv_w2, self.pv_p2(own)) + self.pv_ctx(cls).unsqueeze(1))
        score = self.pv_out(z).squeeze(-1)
        if hasattr(self, "pv_pair_a"):
            score = score + self._pv_pair(own, present[:, :6])
        if self.softcap:
            score = self.softcap_v * torch.tanh(score / self.softcap_v)
        return score, self.pv_value(cls).squeeze(-1)


def build_net(vocab, n_act, arch):
    """The network for an architecture description (from `arch_of` or the command line)."""
    a = dict(arch)
    if a.pop("kind", "mlp") == "transformer":
        net = TNet(vocab, n_act, d_model=a["d_model"], layers=a["layers"], ffn=a["ffn"], obs_dim=a.get("obs_dim", OBS_T),
                    tokens=a.get("tokens", 25), history=a.get("history", False), activation=a.get("activation", "relu"),
                    dec_layers=a.get("dec_layers", 2), vbins=a.get("vbins", 0), enc_opts=a.get("enc_opts", 0),
                    softcap=a.get("softcap", 0.0))
        if a.get("future"):
            net.add_future_heads()
        if a.get("joint"):
            net.add_joint_head()
        if a.get("belief"):
            net.add_belief_head()
        if a.get("unc"):
            net.add_unc_input()
            import encoding
            encoding.ensure_unc_library()      # the unc features are zeros in this process without the set library
        if a.get("ext"):
            net.add_ext_input()
        if a.get("pvpair"):
            net.add_preview_pair()
        if a.get("oppjoint"):
            net.add_opp_joint()
        if a.get("oppcond"):
            net.add_opp_cond()
        if a.get("typebias"):
            net.add_type_bias()
        if a.get("v3heads"):
            net.add_v3_heads()
        if a.get("belin"):
            net.add_belief_input()
        if a.get("memory"):
            net.add_memory()
        return net
    return Net(vocab, n_act, **a)


def masked_logits(logits, mask):
    return logits.masked_fill(mask == 0, -1e9)


def has_joint(net):
    return getattr(getattr(net, "model", net), "jt_a", None) is not None


def has_opp_joint(net):
    return getattr(getattr(net, "model", net), "oj_a", None) is not None


def opp_pair_bad(device):
    """[38, 38] structurally impossible (slot 1, slot 2) pairs: both switching to the same Pokemon, both Mega Evolving
    (pair_mask's rules without a legal mask: the opponent's mask isn't stored with the labels)."""
    ar = torch.arange(38, device=device)
    same_sw = (ar[:, None] == ar[None, :]) & (ar[:, None] >= 32) & (ar[:, None] < 36)
    mega = (ar < 32) & (ar % 2 == 1)
    return same_sw | (mega[:, None] & mega[None, :])


def opp_joint_logprobs(ol, oj):
    """log-probabilities [b, 38, 38] of the opponent's (slot 1, slot 2) pairs from the marginal logits ol [b, 2, 38] and the
    interaction oj [b, 38, 38] (None = the product of the marginals), impossible pairs excluded"""
    x = ol[:, 0, :, None].float() + ol[:, 1, None, :].float()
    if oj is not None:
        x = x + oj.float()
    x = x.masked_fill(opp_pair_bad(x.device), -1e9)
    return LSM(x.flatten(1), -1).view_as(x)


def pair_mask(m):
    """[b, 38, 38] legal (slot 1, slot 2) pairs: both legal, not both switching to the same Pokemon, not both Mega
    Evolving. A row with no legal pair (shouldn't happen) falls back to all pairs of legal actions."""
    m = m.bool()
    pm = m[:, 0, :, None] & m[:, 1, None, :]
    ar = torch.arange(38, device=m.device)
    same_sw = (ar[:, None] == ar[None, :]) & (ar[:, None] >= 32) & (ar[:, None] < 36)
    mega = (ar < 32) & (ar % 2 == 1)
    bad = same_sw | (mega[:, None] & mega[None, :])
    pm2 = pm & ~bad
    ok = pm2.flatten(1).any(1)
    return torch.where(ok[:, None, None], pm2, pm)


def joint_logprobs(logits, m, inter=None):
    """log-probabilities [b, 38, 38] of the (slot 1, slot 2) pairs. Without an interaction term this is the product of
    the two slots' softmaxes, restricted to the legal pairs (pair_mask) and renormalised."""
    lg = logits.float()
    x = lg[:, 0, :, None] + lg[:, 1, None, :]
    if inter is not None:
        x = x + inter.float()
    x = x.masked_fill(~pair_mask(m), -1e9)
    return LSM(x.flatten(1), -1).view_as(x)


@torch.no_grad()
def policy_gpu(net, of, oi, m, greedy=False):
    """Actions (Gumbel-max sampling = sampling from the softmax, cheaper than torch.distributions),
    their summed log-probability over both slots, and the value, all on the GPU."""
    if has_joint(net):
        # (2026-10-19) joint head: one draw over the 38 x 38 legal pairs
        with amp():
            logits, v, inter = net(of, oi, joint=True)
        L = joint_logprobs(logits, m, inter).flatten(1)
        if greedy:
            k = L.argmax(-1)
        else:
            u = torch.rand_like(L).clamp_(1e-9, 1.0)
            k = (L - torch.log(-torch.log(u))).argmax(-1)
        a = torch.stack([k // 38, k % 38], -1)
        return a, L.gather(-1, k.unsqueeze(-1)).squeeze(-1), v.float()
    with amp():
        logits, v = net(of, oi)
    lg = masked_logits(logits.float(), m)
    if greedy:
        a = lg.argmax(-1)
    else:
        u = torch.rand_like(lg).clamp_(1e-9, 1.0)
        a = (lg - torch.log(-torch.log(u))).argmax(-1)
    logp = torch.log_softmax(lg, -1).gather(-1, a.unsqueeze(-1)).squeeze(-1).sum(-1)
    return a, logp, v.float()


@torch.no_grad()
def preview_gpu(net, pf, pi, pm, greedy=False):
    """Team preview option (sampled or best), its log-probability and the matchup value, on the GPU."""
    with amp():
        logits, v = net.preview(pf, pi)
    lg = logits.float().masked_fill(pm == 0, -1e9)
    if greedy:
        a = lg.argmax(-1)
    else:
        u = torch.rand_like(lg).clamp_(1e-9, 1.0)
        a = (lg - torch.log(-torch.log(u))).argmax(-1)
    logp = torch.log_softmax(lg, -1).gather(-1, a.unsqueeze(-1)).squeeze(-1)
    return a, logp, v.float()
