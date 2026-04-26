use std::time::SystemTime;

use chrono::{DateTime, Local};

/// 时间显示格式：24 小时制绝对时间。
const TIME_FORMAT: &str = "%H:%M:%S";

/// 获取当前系统时间戳。
#[must_use]
pub fn now_timestamp() -> SystemTime {
    SystemTime::now()
}

/// 将时间戳渲染为本地绝对时间。
#[must_use]
pub fn format_absolute_timestamp(timestamp: SystemTime) -> String {
    let local_time: DateTime<Local> = DateTime::<Local>::from(timestamp);
    local_time.format(TIME_FORMAT).to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_absolute_timestamp, now_timestamp};

    #[test]
    fn format_absolute_timestamp_shape_is_stable() {
        let rendered = format_absolute_timestamp(now_timestamp());
        assert_eq!(rendered.len(), 8);
        assert_eq!(rendered.chars().nth(2), Some(':'));
        assert_eq!(rendered.chars().nth(5), Some(':'));
    }
}
