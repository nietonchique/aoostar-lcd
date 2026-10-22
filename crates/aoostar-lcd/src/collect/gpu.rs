//! GPU 0 utilisation, temperature, power and memory via `nvidia-smi`.

use std::time::Duration;

use anyhow::{Context, anyhow, bail};

use crate::model::Gpu;

use super::proc;

/// `nvidia-smi` can hang when the GPU is wedged.
const NVIDIA_SMI_TIMEOUT: Duration = Duration::from_secs(3);

pub fn read() -> anyhow::Result<Gpu> {
    let output = proc::run(
        "nvidia-smi",
        &[
            "--id=0",
            "--query-gpu=utilization.gpu,temperature.gpu,power.draw,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ],
        NVIDIA_SMI_TIMEOUT,
    )
    .context("nvidia-smi")?;
    if !output.success {
        bail!("nvidia-smi failed");
    }
    let line = output
        .stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| anyhow!("nvidia-smi printed nothing"))?;
    parse_gpu(line).ok_or_else(|| anyhow!("unexpected nvidia-smi output"))
}

/// One CSV line: `util %, temp C, power W, mem used MiB, mem total MiB`.
/// Fields the driver cannot report are `[N/A]` / `[Not Supported]`.
pub fn parse_gpu(line: &str) -> Option<Gpu> {
    let fields: Vec<Option<f64>> = line
        .split(',')
        .map(|field| {
            let field = field.trim();
            if field.starts_with('[') || field.eq_ignore_ascii_case("n/a") {
                None
            } else {
                field.parse::<f64>().ok().filter(|v| v.is_finite())
            }
        })
        .collect();
    if fields.len() < 5 {
        return None;
    }
    Some(Gpu {
        util_percent: fields[0].map(|v| v as f32),
        temp_c: fields[1].map(|v| v as f32),
        power_w: fields[2].map(|v| v as f32),
        mem_used_mib: fields[3].map(|v| v.max(0.0).round() as u64),
        mem_total_mib: fields[4].map(|v| v.max(0.0).round() as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_line() {
        let gpu = parse_gpu("0, 43, 16.22, 32122, 32607").unwrap();
        assert_eq!(gpu.util_percent, Some(0.0));
        assert_eq!(gpu.temp_c, Some(43.0));
        assert!((gpu.power_w.unwrap() - 16.22).abs() < 0.001);
        assert_eq!(gpu.mem_used_mib, Some(32_122));
        assert_eq!(gpu.mem_total_mib, Some(32_607));
    }

    #[test]
    fn unavailable_fields_become_none() {
        let gpu = parse_gpu("12, 50, [N/A], [Not Supported], 32607").unwrap();
        assert_eq!(gpu.util_percent, Some(12.0));
        assert_eq!(gpu.power_w, None);
        assert_eq!(gpu.mem_used_mib, None);
        assert_eq!(gpu.mem_total_mib, Some(32_607));
    }

    #[test]
    fn short_or_garbage_lines_are_rejected() {
        assert!(parse_gpu("1, 2, 3").is_none());
        assert!(parse_gpu("").is_none());
    }
}
