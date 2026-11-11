#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Unauthorized,
    RateLimited { retry_after_secs: Option<u64> },
    NoCredit,
    ModelUnavailable,
    ContextTooLong,
    Network(String),
    Other { status: u16, message: String },
}
