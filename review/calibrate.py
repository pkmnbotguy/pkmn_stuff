"""Value-net output -> calibrated win probability, from calibration.json (fitted on the network's own self-play games:
isotonic knots for battle positions, logistic for team preview)."""
import numpy as np


def apply_cal(c, v):
    """Calibrated win probability of value-net output(s) v (c: one kind's entry of calibration.json)."""
    if "knots_v" in c:
        return np.interp(v, c["knots_v"], c["knots_p"])
    return 1 / (1 + np.exp(-(c["a"] * np.asarray(v) + c["b"])))
