use super::{Result, Store, now};
use rusqlite::params;
use time::OffsetDateTime;

/// Start of the UTC calendar month containing `at`, and of the month after.
fn month_bounds(at: i64) -> (i64, i64) {
    let t = OffsetDateTime::from_unix_timestamp(at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let (y, m) = (t.year(), t.month());
    let start = time::Date::from_calendar_date(y, m, 1)
        .expect("day 1 exists")
        .midnight()
        .assume_utc();
    let (ny, nm) = if m == time::Month::December {
        (y + 1, time::Month::January)
    } else {
        (y, m.next())
    };
    let end = time::Date::from_calendar_date(ny, nm, 1)
        .expect("day 1 exists")
        .midnight()
        .assume_utc();
    (start.unix_timestamp(), end.unix_timestamp())
}

impl Store {
    pub fn record_usage(&self, chat_id: &str, requests: u32, cost_usd: f64) -> Result<()> {
        self.record_usage_at(chat_id, requests, cost_usd, now())
    }

    pub fn record_usage_at(
        &self,
        chat_id: &str,
        requests: u32,
        cost_usd: f64,
        at: i64,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO usage (chat_id, requests, cost_usd, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![chat_id, requests, cost_usd, at],
        )?;
        Ok(())
    }

    /// Total requests and cost for one chat.
    pub fn chat_usage(&self, chat_id: &str) -> Result<(u32, f64)> {
        self.conn().query_row(
            "SELECT COALESCE(SUM(requests), 0), COALESCE(SUM(cost_usd), 0.0) FROM usage WHERE chat_id = ?1",
            [chat_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    /// Spend across all chats in the UTC calendar month containing `now`.
    pub fn month_spend(&self, now: i64) -> Result<f64> {
        let (start, end) = month_bounds(now);
        self.conn().query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM usage WHERE created_at >= ?1 AND created_at < ?2",
            params![start, end],
            |r| r.get(0),
        )
    }
}
