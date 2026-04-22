"""
Calibration runner — fits (beta_f, sigma_f) per (station, month, lead_bin)
from historical forecast vs settlement data and writes calibration.json.

Run this weekly (or after accumulating new settlement data).

Usage:
    python calibrate.py
    python calibrate.py --db path/to/data.db --out path/to/calibration.json
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

# Delegate entirely to the analysis module
from analysis.calibration import main

if __name__ == "__main__":
    main()
