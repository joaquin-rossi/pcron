//! the PCRON_ variables of a tab

use anyhow::{Context, Result, anyhow, bail};
use std::time::Duration;

use super::{eval_arg, lexer, parser};
use crate::{DelayBounds, Settings, window::Zone};

/// the variables of the tab that configure pcron instead of the scripts
pub const SETTINGS: [&str; 6] = [
    "PCRON_MIN_DELAY",
    "PCRON_MAX_DELAY",
    "PCRON_TIMEOUT",
    "PCRON_SHUTDOWN_TIMEOUT",
    "PCRON_TZ",
    "PCRON_ON_FAILURE",
];

// the PCRON_ variables in effect for a command; an empty one means the default
pub(super) fn settings(env: &[(String, String)]) -> Result<Settings> {
    let mut settings = Settings::default();
    let (mut min, mut max) = (settings.bounds.min, settings.bounds.max);
    for (name, value) in env.iter().filter(|(name, _)| name.starts_with("PCRON_")) {
        let in_setting = || format!("in `{name}={value}`");
        let secs = || seconds(value).with_context(in_setting);
        let duration = |secs: f64| {
            Duration::try_from_secs_f64(secs).map_err(|_| anyhow!("{}: too long", in_setting()))
        };
        let text = value.trim();
        match name.as_str() {
            "PCRON_MIN_DELAY" => min = secs()?.unwrap_or(min),
            "PCRON_MAX_DELAY" => max = secs()?.unwrap_or(max),
            // 0 means no limit too
            "PCRON_TIMEOUT" => {
                settings.timeout = secs()?.filter(|&s| s > 0.0).map(duration).transpose()?;
            }
            "PCRON_SHUTDOWN_TIMEOUT" => {
                if let Some(secs) = secs()? {
                    settings.shutdown_timeout = duration(secs)?;
                }
            }
            "PCRON_TZ" if text.is_empty() => settings.tz = Zone::Local,
            "PCRON_TZ" => {
                settings.tz = Zone::parse(text).map_err(|e| anyhow!("{}: {e}", in_setting()))?;
            }
            "PCRON_ON_FAILURE" => {
                settings.on_failure = Some(text.to_string()).filter(|t| !t.is_empty());
            }
            _ => bail!(
                "unknown setting `{name}` (the PCRON_ ones are {})",
                SETTINGS.join(", ")
            ),
        }
    }
    settings.bounds = DelayBounds::new(min, max).map_err(|e| anyhow!(e))?;
    Ok(settings)
}

// a constant expression, like the arguments of a distribution; None if empty
fn seconds(value: &str) -> Result<Option<f64>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    // the value has a single line, so leave out the "line 1" of the messages
    let unline = |e: String| anyhow!(e.strip_prefix("line 1: ").unwrap_or(&e).to_string());
    let tokens = lexer::tokenize(value).map_err(unline)?;
    let expr = parser::parse_expr(&tokens).map_err(unline)?;
    eval_arg(&expr).map(Some)
}
