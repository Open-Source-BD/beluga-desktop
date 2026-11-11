pub const APP_NAME: &str = "Beluga";
pub const KEYCHAIN_SERVICE: &str = "app.beluga.desktop";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_name_is_beluga() {
        assert_eq!(APP_NAME, "Beluga");
    }

    #[test]
    fn keychain_service_has_no_spaces() {
        assert!(!KEYCHAIN_SERVICE.contains(' '));
    }
}
