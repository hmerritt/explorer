use std::time::SystemTime;

use chrono::{DateTime, Local};

use crate::explorer::constants::{GB_BYTES, KB_BYTES, MB_BYTES, TB_BYTES};

pub(super) fn format_timestamp(timestamp: Option<SystemTime>) -> String {
    let Some(timestamp) = timestamp else {
        return String::new();
    };

    let local: DateTime<Local> = timestamp.into();
    local.format("%Y/%m/%d %H:%M").to_string()
}

pub(super) fn format_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };

    if size < KB_BYTES {
        return format!("{} bytes", format_u64_with_commas(size));
    }

    let (value, precision, unit) = if size < MB_BYTES {
        (size as f64 / KB_BYTES as f64, 1, "KB")
    } else if size < GB_BYTES {
        (size as f64 / MB_BYTES as f64, 2, "MB")
    } else if size < TB_BYTES {
        (size as f64 / GB_BYTES as f64, 2, "GB")
    } else {
        (size as f64 / TB_BYTES as f64, 2, "TB")
    };

    format!("{} {unit}", format_decimal_with_commas(value, precision))
}

fn format_decimal_with_commas(value: f64, precision: usize) -> String {
    let formatted = format!("{value:.precision$}");
    let Some((integer, fraction)) = formatted.split_once('.') else {
        return format_integer_string_with_commas(&formatted);
    };

    format!(
        "{}.{}",
        format_integer_string_with_commas(integer),
        fraction
    )
}

fn format_u64_with_commas(value: u64) -> String {
    format_integer_string_with_commas(&value.to_string())
}

fn format_integer_string_with_commas(value: &str) -> String {
    let mut formatted = String::with_capacity(value.len() + value.len() / 3);

    for (ix, ch) in value.chars().rev().enumerate() {
        if ix > 0 && ix % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(ch);
    }

    formatted.chars().rev().collect()
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::constants::{GB_BYTES, KB_BYTES, MB_BYTES, TB_BYTES};
    use chrono::{Local, TimeZone};

    #[test]
    fn folders_render_blank_size() {
        assert_eq!(format_size(None), "");
    }

    #[test]
    fn files_below_kilobytes_render_as_bytes() {
        assert_eq!(format_size(Some(0)), "0 bytes");
        assert_eq!(format_size(Some(350)), "350 bytes");
        assert_eq!(format_size(Some(1023)), "1,023 bytes");
    }

    #[test]
    fn kilobytes_render_with_one_decimal_place() {
        assert_eq!(format_size(Some(KB_BYTES)), "1.0 KB");
        assert_eq!(format_size(Some(KB_BYTES + 512)), "1.5 KB");
        assert_eq!(format_size(Some(MB_BYTES - 1)), "1,024.0 KB");
    }

    #[test]
    fn megabytes_gigabytes_and_terabytes_render_with_two_decimal_places() {
        assert_eq!(format_size(Some(MB_BYTES)), "1.00 MB");
        assert_eq!(format_size(Some(MB_BYTES + 512 * KB_BYTES)), "1.50 MB");
        assert_eq!(format_size(Some(GB_BYTES)), "1.00 GB");
        assert_eq!(format_size(Some(GB_BYTES + 512 * MB_BYTES)), "1.50 GB");
        assert_eq!(format_size(Some(TB_BYTES)), "1.00 TB");
        assert_eq!(format_size(Some(TB_BYTES + 512 * GB_BYTES)), "1.50 TB");
    }

    #[test]
    fn large_file_sizes_include_commas_and_stay_capped_at_terabytes() {
        assert_eq!(format_size(Some(1024 * MB_BYTES)), "1.00 GB");
        assert_eq!(format_size(Some(1024 * GB_BYTES)), "1.00 TB");
        assert_eq!(format_size(Some(1024 * TB_BYTES)), "1,024.00 TB");
    }

    #[test]
    fn timestamp_uses_local_explorer_format() {
        let local = Local.with_ymd_and_hms(2026, 5, 31, 21, 48, 12).unwrap();
        assert_eq!(format_timestamp(Some(local.into())), "2026/05/31 21:48");
    }
}
