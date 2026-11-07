"""
(2026-10-18, speed idea #11; UNTESTED: Triton needs Linux + a GPU of compute capability 7.0+, e.g. the 5090; the GTX 1070
and Windows can't run it) TNET_ATTN=triton: the encoder's attention as ONE fused Triton kernel per layer for inference
(no gradient): one program per (game, head) holds the whole 17-token attention in registers (tokens padded to 32, not
PyTorch's 64 x 64 tiles), q k^T, mask, softmax and x v in one launch instead of ~6. Training (grad enabled) keeps the
written-out "manual" attention, so the learning maths is unchanged.

Safety: the first call compares the kernel with the plain computation on the same input; a difference above 2e-3
(fp32; 3e-2 under bf16) or any error turns it off for the rest of the process with a message ("manual" from then on).
Measure games/s with TNET_ATTN=manual vs triton on the rental before keeping it.
"""

import torch

STATE = {"ok": None, "checked": False}

try:
    import triton
    import triton.language as tl
except Exception:          # no Triton (Windows, CPU-only installs)
    triton = None

if triton is not None:
    @triton.jit
    def _attn_kernel(Q, K, V, M, O, sqb, sqh, sql, smb, sob, soh, sol, L, scale,
                     BL: tl.constexpr, HD: tl.constexpr, IEEE: tl.constexpr):
        b = tl.program_id(0)
        h = tl.program_id(1)
        ol = tl.arange(0, BL)
        od = tl.arange(0, HD)
        lm = ol < L
        base = b * sqb + h * sqh
        ptr = base + ol[:, None] * sql + od[None, :]
        q = tl.load(Q + ptr, mask=lm[:, None], other=0.0)
        k = tl.load(K + ptr, mask=lm[:, None], other=0.0)
        v = tl.load(V + ptr, mask=lm[:, None], other=0.0)
        if IEEE:
            s = tl.dot(q, tl.trans(k), input_precision="ieee")
        else:
            s = tl.dot(q, tl.trans(k))
        s = s.to(tl.float32) * scale
        keep = tl.load(M + b * smb + ol, mask=lm, other=0)
        s = tl.where(((keep != 0) & lm)[None, :], s, float("-inf"))
        mx = tl.max(s, 1)
        p = tl.exp(s - mx[:, None])
        p = p / tl.sum(p, 1)[:, None]
        if IEEE:
            o = tl.dot(p.to(v.dtype), v, input_precision="ieee")
        else:
            o = tl.dot(p.to(v.dtype), v)
        tl.store(O + b * sob + h * soh + ol[:, None] * sol + od[None, :], o.to(O.dtype.element_ty), mask=lm[:, None])


def reference(q, k, v, keep, scale):
    """The 'manual' attention (netlib.TNet._encode) on [b, H, L, hd] tensors and a [b, L] bool keep mask."""
    sc = torch.matmul(q, k.transpose(-1, -2))
    sc = (sc.float() if sc.dtype in (torch.float16, torch.bfloat16) else sc) * scale
    sc = sc.masked_fill(~keep[:, None, None, :], float("-inf"))
    return torch.matmul(torch.softmax(sc, -1).to(v.dtype), v)


def _launch(q, k, v, keep, scale):
    b, H, L, hd = q.shape
    o = torch.empty(b, H, L, hd, device=q.device, dtype=q.dtype)
    m = keep.to(torch.int8).contiguous()
    BL = max(16, triton.next_power_of_2(L))
    _attn_kernel[(b, H)](q, k, v, m, o, q.stride(0), q.stride(1), q.stride(2), m.stride(0),
                         o.stride(0), o.stride(1), o.stride(2), L, scale,
                         BL=BL, HD=hd, IEEE=(q.dtype == torch.float32), num_warps=4)
    return o


def usable(q):
    if STATE["ok"] is False or triton is None or not q.is_cuda or torch.is_grad_enabled():
        return False
    hd = q.shape[-1]
    return hd >= 16 and hd & (hd - 1) == 0 and q.stride(-1) == 1 and q.stride() == q.stride()


def attn(q, k, v, keep, scale):
    """Fused attention, or the reference when the kernel can't apply / failed its check."""
    if not usable(q) or k.stride() != q.stride() or v.stride() != q.stride():
        return reference(q, k, v, keep, scale)
    try:
        o = _launch(q, k, v, keep, scale)
        if not STATE["checked"]:
            r = reference(q, k, v, keep, scale)
            diff = (o.float() - r.float()).nan_to_num(0.0).abs().max().item()
            tol = 2e-3 if q.dtype == torch.float32 else 3e-2
            STATE["checked"] = True
            STATE["ok"] = diff <= tol
            print(f"TNET_ATTN=triton: self-check max difference {diff:.2e} -> {'on' if STATE['ok'] else 'OFF (manual)'}", flush=True)
            if not STATE["ok"]:
                return r
        return o
    except Exception as e:
        STATE["ok"] = False
        print(f"TNET_ATTN=triton: kernel failed ({type(e).__name__}: {e}); using manual attention", flush=True)
        return reference(q, k, v, keep, scale)
