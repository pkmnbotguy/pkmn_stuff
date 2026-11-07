"""
(2026-10-18 speed, idea #13) Faster network calls at play time (agent / review).

On the GTX 1070 a forward pass of a few rows is dominated by kernel launches (~4.8 ms for 1 row, ~1.4 ms replayed from a
CUDA graph); from ~128 rows on the GPU is compute-bound and graphs change nothing (fp16 is 1/64 speed on Pascal and
TensorRT 10 dropped it, so there is nothing faster there). `wrap(net)` therefore replays CUDA graphs for calls of up to
SMALL_MAX rows and runs bigger ones normally. Outputs are identical to the plain network (measured max difference 0) and
are cloned, so callers may keep them. Preview calls go to the plain network. (2026-10-23) Calls with the keyword sets in
GRAPH_KW (the search's root call: opp=True, opp=True + opp_joint=True, joint=True) get graphs of their own, captured on
first use; other keywords run the plain network.

    net = fastinfer.wrap(net)        # no-op on the CPU or with VGC_CUDA_GRAPHS=0

Thread-safe (one lock per wrapped net): the review server calls the net from worker threads.
"""

import os
import threading
import torch

SMALL_MAX = int(os.environ.get("VGC_GRAPH_MAX", "64"))
# (2026-10-28) graph sizes between the powers of two: a call is padded up to the next size, so with only powers of two a
# 33-row call paid for 64 (the deepening's mean call is ~42 rows). VGC_GRAPH_SIZES="1,2,4,..." overrides; "pow2" = old.
_GS = os.environ.get("VGC_GRAPH_SIZES", "")
if _GS == "pow2":
    _GS = "1,2,4,8,16,32,64,128,256"
SIZES = [s for s in (sorted(int(x) for x in _GS.split(",")) if _GS else
                     (1, 2, 4, 6, 8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256))
         if s <= SMALL_MAX]
# keyword sets replayed from graphs (sorted (name, value) tuples); () = the plain (logits, value) call
GRAPH_KW = {(), (("opp", True),), (("opp", True), ("opp_joint", True)), (("joint", True),)}


class GraphNet(torch.nn.Module):
    def __init__(self, net, device, width):
        super().__init__()
        self.net = net
        self.device = device
        self.width = width
        self.g = None
        self.lock = threading.Lock()
        self.pool = None

    # the wrapped net's attributes (obs_dim, preview, parameters...) stay reachable
    def __getattr__(self, name):
        try:
            return super().__getattr__(name)
        except AttributeError:
            return getattr(self.__dict__["_modules"]["net"], name)

    def _capture(self, i_dim, key=()):
        from netlib import amp
        if self.g is None:
            self.g = {}
        kw = dict(key)
        gk = {}
        for size in sorted(SIZES, reverse=True):
            sf = torch.zeros(size, self.width, device=self.device)
            si = torch.zeros(size, i_dim, dtype=torch.int32, device=self.device)
            sf[:, 0] = 1.0
            side = torch.cuda.Stream()
            side.wait_stream(torch.cuda.current_stream())
            with torch.cuda.stream(side), torch.no_grad(), amp():
                for _ in range(2):
                    self.net(sf, si, **kw)
            torch.cuda.current_stream().wait_stream(side)
            g = torch.cuda.CUDAGraph()
            with torch.no_grad(), amp(graph=True), torch.cuda.graph(g, pool=self.pool):
                out = self.net(sf, si, **kw)
            if self.pool is None:
                self.pool = g.pool()
            gk[size] = (g, sf, si, out)
        self.g[key] = gk
        torch.cuda.synchronize()

    def forward(self, f, i, opp=False, **kw):
        n = f.shape[0]
        if self.width is None:
            self.width = f.shape[1]        # the encoder's feature count (old nets read a prefix of it)
        key = tuple(sorted(((k, v) for k, v in dict(kw, opp=opp).items() if v is not False and v is not None)))
        if key not in GRAPH_KW or n == 0 or n > SIZES[-1] or self.net.training or f.shape[1] != self.width or torch.is_grad_enabled():
            return self.net(f, i, opp=opp, **kw) if (opp or kw) else self.net(f, i)
        with self.lock:
            if self.g is None or key not in self.g:
                self._capture(i.shape[1], key)
            size = next(s for s in SIZES if s >= n)
            g, sf, si, out = self.g[key][size]
            sf[:n].copy_(f)
            si[:n].copy_(i)
            if n < size:
                sf[n:].zero_()
                sf[n:, 0] = 1.0
                si[n:].zero_()
            g.replay()
            return tuple(o[:n].clone() if o is not None else None for o in out)


def wrap(net, width=None):
    """The net with small calls replayed from CUDA graphs (see the module doc); the net itself if that can't apply."""
    if os.environ.get("VGC_CUDA_GRAPHS", "1") == "0" or isinstance(net, GraphNet):
        return net
    try:
        dev = next(net.parameters()).device
    except StopIteration:
        return net
    if dev.type != "cuda":
        return net
    # (2026-10-28) play-time nets use the fused attention path (F.scaled_dot_product_attention, PyTorch's kernel choice):
    # same outputs to ~1e-5, +13% positions per 4 s of deepening for v2 on the 1070 (with the dense graph sizes). Only when
    # TNET_ATTN isn't set (it also picks the TRAINING path); VGC_PLAY_ATTN overrides, "" = keep the class default.
    play_attn = os.environ.get("VGC_PLAY_ATTN", "fused")
    if "TNET_ATTN" not in os.environ and play_attn and hasattr(net, "attn_impl"):
        net.attn_impl = play_attn
    return GraphNet(net, dev, width)
