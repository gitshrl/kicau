//! Read the X session cookies from an installed browser, so `kicau login` does
//! not need them pasted by hand. Locating each browser's cookie store and
//! decrypting Chromium values is cookie-scoop's job; this module turns its flat
//! cookie list into the `(browser, auth_token, ct0)` sessions kicau logs in with.

use std::collections::HashMap;

use cookie_scoop::get_cookies;
use cookie_scoop::types::{BrowserName, Cookie, GetCookiesOptions};

/// One browser profile the cookies came from.
type SessionKey = (BrowserName, Option<String>);
/// The two cookies for a session, either still missing until both are seen.
type CookiePair = (Option<String>, Option<String>);

/// Whether a graphical session is present, given the platform and the display
/// environment. Pure inputs so it is testable without touching real env vars.
fn has_gui(is_macos: bool, display: Option<&str>, wayland: Option<&str>) -> bool {
    // macOS is a desktop OS: its browser cookie stores sit on disk and stay
    // readable even over SSH, so there is always something to read.
    is_macos || display.is_some_and(|d| !d.is_empty()) || wayland.is_some_and(|w| !w.is_empty())
}

/// Whether this machine has no graphical session, so no browser is available to
/// read cookies from and `kicau login` must go straight to a manual paste.
pub fn is_headless() -> bool {
    let display = std::env::var("DISPLAY").ok();
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    !has_gui(
        cfg!(target_os = "macos"),
        display.as_deref(),
        wayland.as_deref(),
    )
}

/// An X session found in a browser: the two cookies kicau needs and where they
/// came from.
pub struct BrowserSession {
    pub browser: BrowserName,
    pub profile: Option<String>,
    pub auth_token: String,
    pub ct0: String,
}

impl BrowserSession {
    /// A label for the selection menu: `chrome` or `chrome (Profile 1)`.
    pub fn label(&self) -> String {
        match &self.profile {
            Some(profile) => format!("{} ({profile})", self.browser),
            None => self.browser.to_string(),
        }
    }
}

/// Group cookie-scoop's flat list into sessions: a `(browser, profile)` pair
/// that carries BOTH `auth_token` and `ct0`. One cookie without the other is
/// not a login. Pure over the extracted cookies, so it needs no browser to test.
fn sessions_from_cookies(cookies: &[Cookie]) -> Vec<BrowserSession> {
    let mut groups: HashMap<SessionKey, CookiePair> = HashMap::new();
    for cookie in cookies {
        let Some(source) = &cookie.source else {
            continue;
        };
        if cookie.value.is_empty() {
            continue;
        }
        let slot = groups
            .entry((source.browser, source.profile.clone()))
            .or_default();
        match cookie.name.as_str() {
            "auth_token" => slot.0 = Some(cookie.value.clone()),
            "ct0" => slot.1 = Some(cookie.value.clone()),
            _ => {}
        }
    }
    let mut sessions: Vec<BrowserSession> = groups
        .into_iter()
        .filter_map(|((browser, profile), (auth_token, ct0))| {
            Some(BrowserSession {
                browser,
                profile,
                auth_token: auth_token?,
                ct0: ct0?,
            })
        })
        .collect();
    // Stable order so the menu and the auto-pick are predictable.
    sessions.sort_by_key(BrowserSession::label);
    sessions
}

/// Scan installed browsers for logged-in X sessions. A browser cookie-scoop
/// cannot read (absent, locked keyring, Safari without Full Disk Access) becomes
/// a warning on stderr and contributes nothing; it never stops the scan.
pub async fn scan_sessions() -> Vec<BrowserSession> {
    let options = GetCookiesOptions::new("https://x.com")
        .names(vec!["auth_token".to_string(), "ct0".to_string()])
        .browsers(vec![
            BrowserName::Chrome,
            BrowserName::Edge,
            BrowserName::Firefox,
            BrowserName::Safari,
        ]);
    let result = get_cookies(options).await;
    for warning in &result.warnings {
        // A browser that is simply not installed is expected, not worth a warning.
        if warning.to_lowercase().contains("not found") {
            continue;
        }
        eprintln!("⚠️  browser cookies: {warning}");
    }
    sessions_from_cookies(&result.cookies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cookie_scoop::types::CookieSource;

    fn cookie(name: &str, value: &str, browser: BrowserName, profile: Option<&str>) -> Cookie {
        Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain: Some(".x.com".to_string()),
            path: None,
            url: None,
            expires: None,
            secure: None,
            http_only: None,
            same_site: None,
            source: Some(CookieSource {
                browser,
                profile: profile.map(str::to_string),
                origin: None,
                store_id: None,
            }),
        }
    }

    #[test]
    fn a_session_needs_both_cookies() {
        let cookies = vec![
            cookie("auth_token", "aaa", BrowserName::Chrome, None),
            cookie("ct0", "ccc", BrowserName::Chrome, None),
            // Firefox has only auth_token: not a session.
            cookie("auth_token", "bbb", BrowserName::Firefox, None),
        ];
        let sessions = sessions_from_cookies(&cookies);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].browser, BrowserName::Chrome);
        assert_eq!(sessions[0].auth_token, "aaa");
        assert_eq!(sessions[0].ct0, "ccc");
    }

    #[test]
    fn sessions_are_scoped_per_profile() {
        let cookies = vec![
            cookie("auth_token", "a1", BrowserName::Chrome, Some("Default")),
            cookie("ct0", "c1", BrowserName::Chrome, Some("Default")),
            cookie("auth_token", "a2", BrowserName::Chrome, Some("Profile 1")),
            cookie("ct0", "c2", BrowserName::Chrome, Some("Profile 1")),
        ];
        assert_eq!(sessions_from_cookies(&cookies).len(), 2);
    }

    #[test]
    fn an_empty_cookie_value_is_not_a_session() {
        let cookies = vec![
            cookie("auth_token", "", BrowserName::Chrome, None),
            cookie("ct0", "ccc", BrowserName::Chrome, None),
        ];
        assert!(sessions_from_cookies(&cookies).is_empty());
    }

    #[test]
    fn gui_detection_by_platform_and_display() {
        // macOS: always a GUI for reading cookies, display env aside.
        assert!(has_gui(true, None, None));
        // Linux server with no display: headless.
        assert!(!has_gui(false, None, None));
        // An empty DISPLAY is not a session.
        assert!(!has_gui(false, Some(""), None));
        // Linux desktop: X11 or Wayland.
        assert!(has_gui(false, Some(":0"), None));
        assert!(has_gui(false, None, Some("wayland-0")));
    }
}
