//! NL→cron scheduling (ADR-20260621 slice 4d). The LLM PROPOSES a cron expression; this
//! module's deterministic validator GATES it — an unparseable, wrong-arity, or
//! sub-minimum-interval (`* * * * *` DoS) expression is rejected in pure Rust. cron/chrono are
//! implementation details: the public API is i64 unix-seconds + String, so callers (the bin,
//! sa-memory) never grow a chrono dependency. Cron is interpreted in UTC.
//! ponytail: UTC-only; a per-job timezone column is the upgrade if local-time intent matters.

use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use cron::Schedule;
use sa_providers::{ChatMsg, Provider};
use std::str::FromStr;

/// A scheduled job must not fire more often than this. `* * * * *` (60s) is rejected; the
/// smallest 5-field cron granularity is one minute, so this floor bounds unattended token spend.
/// ponytail: 5 min is a sane assistant-job floor; lower it (or make it per-job) only if a real
/// high-frequency job is needed.
pub const MIN_INTERVAL_SECS: i64 = 300;

/// Parse the model's 5-field cron string into the `cron` crate's seconds-leading 6-field form.
/// Enforces EXACTLY 5 standard fields (minute hour dom month dow) — rejecting 6-field/`@macro`
/// output keeps the validator strict and the seconds-field always 0.
fn to_schedule(expr: &str) -> Result<Schedule> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        bail!(
            "expected a 5-field cron expression, got {} fields",
            fields.len()
        );
    }
    // cron crate is sec-leading 6/7-field; prepend a literal 0-seconds.
    let sixed = format!("0 {}", fields.join(" "));
    Schedule::from_str(&sixed).with_context(|| format!("unparseable cron: {expr}"))
}

/// Normalize + fully validate a 5-field cron expression. Returns the canonical single-spaced
/// form. Rejects bad arity, unparseable fields, and any pattern whose MINIMUM gap between
/// consecutive fires is below `MIN_INTERVAL_SECS` (the DoS floor — catches `* * * * *` and
/// bursty patterns like `0,1 * * * *`).
pub fn validate_cron(expr: &str) -> Result<String> {
    let schedule = to_schedule(expr)?;
    // Min gap over the next several fires from a fixed reference (catches bursty minima, not just
    // the first interval). ponytail: 10 samples bounds the check; widen if a pathological pattern
    // slips a sub-floor gap past the 10th fire.
    let reference = Utc.timestamp_opt(0, 0).single().context("epoch")?;
    let fires: Vec<_> = schedule.after(&reference).take(11).collect();
    if fires.len() < 2 {
        bail!("cron expression never fires");
    }
    let min_gap = fires
        .windows(2)
        .map(|w| (w[1] - w[0]).num_seconds())
        .min()
        .unwrap_or(0);
    if min_gap < MIN_INTERVAL_SECS {
        bail!("schedule fires every {min_gap}s — below the {MIN_INTERVAL_SECS}s minimum");
    }
    Ok(expr.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Unix seconds of the next fire strictly after `after_unix` (UTC).
pub fn next_fire_unix(expr: &str, after_unix: i64) -> Result<i64> {
    let schedule = to_schedule(expr)?;
    let after = Utc
        .timestamp_opt(after_unix, 0)
        .single()
        .context("invalid after_unix")?;
    schedule
        .after(&after)
        .next()
        .map(|dt| dt.timestamp())
        .context("cron expression has no next fire")
}

/// The instruction handed to the model to turn an NL request into a cron expression.
pub fn propose_cron_prompt(nl: &str) -> String {
    format!(
        "Convert this scheduling request into a SINGLE standard 5-field cron expression \
         (minute hour day-of-month month day-of-week), interpreted in UTC. Output ONLY the cron \
         expression on one line — no prose, no @macros, no seconds field.\n\nRequest: {nl}"
    )
}

/// Ask the model for a cron expression, then GATE it through `validate_cron`. The model proposes;
/// the validator decides. Strips surrounding backticks/quotes and takes the first line that is
/// exactly 5 cron-ish tokens (models often add a stray word).
pub async fn nl_to_cron(provider: &dyn Provider, nl: &str) -> Result<String> {
    let reply = provider
        .complete(vec![ChatMsg {
            role: "user".into(),
            content: propose_cron_prompt(nl),
        }])
        .await?;
    let candidate = reply
        .lines()
        .map(|l| {
            l.trim()
                .trim_matches(|c| c == '`' || c == '"' || c == '\'')
                .trim()
        })
        .find(|l| l.split_whitespace().count() == 5)
        .unwrap_or_else(|| reply.trim());
    validate_cron(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sa_providers::{ChatChunk, Provider};

    #[test]
    fn validate_accepts_a_daily_morning_cron() {
        assert_eq!(validate_cron(" 0 7 * * * ").unwrap(), "0 7 * * *");
    }

    #[test]
    fn validate_rejects_garbage_and_wrong_field_count() {
        assert!(validate_cron("not a cron").is_err());
        assert!(validate_cron("0 7 * *").is_err(), "4 fields");
        assert!(
            validate_cron("0 0 7 * * *").is_err(),
            "6 fields (seconds) not accepted"
        );
        assert!(validate_cron("99 7 * * *").is_err(), "minute out of range");
    }

    #[test]
    fn validate_rejects_sub_minimum_interval_dos() {
        assert!(
            validate_cron("* * * * *").is_err(),
            "every-minute is the DoS floor"
        );
        assert!(
            validate_cron("0,1 * * * *").is_err(),
            "bursty 60s gap rejected"
        );
        assert!(
            validate_cron("*/5 * * * *").is_ok(),
            "every 5 min is allowed"
        );
    }

    #[test]
    fn next_fire_is_deterministic_in_utc() {
        // 2026-01-01T00:00:00Z = 1767225600. Next "0 7 * * *" is 2026-01-01T07:00:00Z.
        let after = 1_767_225_600;
        let next = next_fire_unix("0 7 * * *", after).unwrap();
        assert_eq!(next, after + 7 * 3600);
        // strictly after: asking again from the fire time yields the next day
        assert_eq!(next_fire_unix("0 7 * * *", next).unwrap(), next + 24 * 3600);
    }

    struct One(String);
    #[async_trait::async_trait]
    impl Provider for One {
        async fn chat(
            &self,
            _m: Vec<sa_providers::ChatMsg>,
        ) -> anyhow::Result<futures::stream::BoxStream<'static, anyhow::Result<ChatChunk>>>
        {
            let r = self.0.clone();
            Ok(Box::pin(futures::stream::once(
                async move { Ok(ChatChunk(r)) },
            )))
        }
    }

    #[tokio::test]
    async fn nl_to_cron_validates_a_well_formed_llm_reply() {
        let p = One("`0 7 * * *`".into()); // model wraps it in backticks
        assert_eq!(
            nl_to_cron(&p, "every morning at 7").await.unwrap(),
            "0 7 * * *"
        );
    }

    #[tokio::test]
    async fn nl_to_cron_rejects_a_bad_llm_reply() {
        let p = One("every minute: * * * * *".into());
        assert!(nl_to_cron(&p, "spam me").await.is_err());
    }
}
