use serde_json::{json, Value};

/// Only standard report plans belong to the analysis settings surface.
/// Titles are persisted, so recognize both locales without renaming existing plans.
pub fn update_body(topic: &Value, period: &str, enabled: Option<bool>, time: Option<&str>) -> Result<Value, String> {
    let titles: &[&str] = match period {
        "day" => &["每日报告", "Daily report", "微信工作日报", "WeChat daily"],
        "week" => &["每周报告", "Weekly report", "微信工作周报", "WeChat weekly"],
        "month" => &["每月报告", "Monthly report", "微信工作月报", "WeChat monthly"],
        _ => return Err("报告周期无效。".into()),
    };
    if topic.get("period").and_then(Value::as_str) != Some(period)
        || !topic.get("title").and_then(Value::as_str).is_some_and(|title| titles.contains(&title)) {
        return Err("无法确认报告计划，请重新读取后重试。".into());
    }
    if enabled.is_none() && time.is_none() { return Err("没有要修改的报告设置。".into()); }
    let mut body = json!({});
    if let Some(enabled) = enabled { body["enabled"] = enabled.into(); }
    if let Some(time) = time {
        if !crate::settings::is_valid_time(time) { return Err("报告时间必须使用 HH:MM 格式。".into()); }
        body["time_local"] = time.into();
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_all_periods_and_persisted_languages() {
        for (period, title) in [("day", "微信工作日报"), ("day", "Daily report"), ("week", "每周报告"), ("week", "WeChat weekly"), ("month", "Monthly report")] {
            assert_eq!(update_body(&json!({"period":period,"title":title}), period, Some(false), None).unwrap(), json!({"enabled":false}));
        }
    }
    #[test]
    fn rejects_wrong_period_custom_topics_and_empty_updates() {
        let plan = json!({"period":"week","title":"每周报告"});
        assert!(update_body(&plan,"day",Some(true),None).is_err());
        assert!(update_body(&plan,"week",None,None).is_err());
        assert!(update_body(&json!({"period":"week","title":"客户跟进"}),"week",Some(true),None).is_err());
        assert!(update_body(&plan,"year",Some(true),None).is_err());
    }
    #[test]
    fn updates_only_requested_fields_and_validates_time() {
        let plan = json!({"period":"month","title":"每月报告","filters":{"keywords":["客户"]},"timezone":"UTC"});
        assert_eq!(update_body(&plan,"month",None,Some("09:15")).unwrap(), json!({"time_local":"09:15"}));
        for invalid in ["24:00", "9:15", "09:60"] { assert!(update_body(&plan,"month",None,Some(invalid)).is_err()); }
    }
}
