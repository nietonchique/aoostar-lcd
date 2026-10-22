# AOOSTAR WTR MAX / GEM12+ PRO UART Screen Protocol

Reverse engineered [AOOSTAR WTR MAX](https://aoostar.com/products/aoostar-wtr-max-amd-r7-pro-8845hs-11-bays-mini-pc)
UART display protocol, written in Rust.  
This project should also support the GEM12+ PRO device.

- [LCD Protocol](docs/lcd_protocol.md)
- Upstream: https://github.com/zehnm/aoostar-rs

## Display Information

- **Resolution:** 960 × 376
- **Manufacturer:** Synwit
- **Connected over USB UART** with a proprietary serial communication protocol:
    - **USB device ID:** `416:90A1` (as shown by `lsusb`)
    - **Linux device (example on Debian):** `/dev/ttyACM0`
    - **1,500,000 baud**, 8N1 (likely ignored; actual USB transfer speed is much higher)
