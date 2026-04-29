//! Cloudflare R2 / S3-compatible object storage.
//!
//! Mirrors the MicroPython implementation in `firmware/lib/s3.py` and
//! `firmware/lib/api/routes_cloud.py`. SigV4 is hand-rolled (see
//! `sigv4.rs`) because we already have `sha2` + `hmac` and do not want to
//! pull a heavy AWS SDK onto the device.

pub mod sigv4;
