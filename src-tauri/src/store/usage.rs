use super::{Result, Store, now};
use rusqlite::{OptionalExtension, params};
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
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO usage (chat_id, requests, cost_usd, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![chat_id, requests, cost_usd, at],
        )?;
        // The per-day total outlives the chat the request was made in.
        tx.execute(
            "INSERT INTO daily_usage (day, requests, cost_usd)
             VALUES (strftime('%Y-%m-%d', ?1, 'unixepoch'), ?2, ?3)
             ON CONFLICT(day) DO UPDATE SET
                 requests = requests + excluded.requests,
                 cost_usd = cost_usd + excluded.cost_usd",
            params![at, requests, cost_usd],
        )?;
        tx.commit()
    }

    /// Total requests and cost for one chat.
    pub fn chat_usage(&self, chat_id: &str) -> Result<(u32, f64)> {
        self.conn().query_row(
            "SELECT COALESCE(SUM(requests), 0), COALESCE(SUM(cost_usd), 0.0) FROM usage WHERE chat_id = ?1",
            [chat_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    /// Requests and cost recorded on one UTC day (`YYYY-MM-DD`), counting
    /// chats that have since been deleted.
    pub fn day_usage(&self, day: &str) -> Result<(u32, f64)> {
        Ok(self
            .conn()
            .query_row(
                "SELECT requests, cost_usd FROM daily_usage WHERE day = ?1",
                [day],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, 0.0)))
    }

    /// Spend across all chats in the UTC calendar month containing `now`.
    pub fn month_spend(&self, now: i64) -> Result<f64> {
        let (start, end) = month_bounds(now);
        self.conn().query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM daily_usage
             WHERE day >= date(?1, 'unixepoch') AND day < date(?2, 'unixepoch')",
            params![start, end],
            |r| r.get(0),
        )
    }
}
