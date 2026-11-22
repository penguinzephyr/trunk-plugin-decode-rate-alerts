use std::{
    collections::HashMap,
    error::Error,
    thread,
    time::{Duration, Instant},
};

use lettre::{Message, SmtpTransport, Transport, message::Mailbox, transport::smtp::authentication::Credentials};
use schemars::JsonSchema;
use serde::Deserialize;
use trunk_recorder_plugin::{Host, Manifest, NoConfig, Plugin, Setup, Status, topic};

const PUSHOVER_URL: &str = "https://api.pushover.net/1/messages.json";

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(default)]
struct Config {
    /// Pushover application token. Leave blank to disable Pushover.
    app_token: String,
    /// Pushover user or group key. Leave blank to disable Pushover.
    user_key: String,
    /// HTTP or HTTPS URL that receives JSON POST notifications. Leave blank to disable the webhook.
    webhook_url: String,
    /// SMTP hostname. Leave blank to disable email.
    smtp_host: String,
    /// SMTP port, commonly 587 for STARTTLS or 465 for TLS.
    smtp_port: u16,
    /// SMTP connection security: "starttls", "tls", or "none".
    smtp_security: String,
    /// SMTP username. Configure with smtp_password when authentication is required.
    smtp_username: String,
    /// SMTP password. Configure with smtp_username when authentication is required.
    smtp_password: String,
    /// Email address notifications are sent from.
    email_from: String,
    /// One or more comma-separated recipient email addresses.
    email_to: String,
    /// Alert when a site's decode rate is below this many messages per second.
    threshold: f64,
    /// Minutes a site must remain below the threshold before an alert is sent.
    alert_after_minutes: u64,
    /// Send a notification when a site's decode rate recovers to the threshold.
    notify_recovery: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_token: String::new(),
            user_key: String::new(),
            webhook_url: String::new(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_security: "starttls".into(),
            smtp_username: String::new(),
            smtp_password: String::new(),
            email_from: String::new(),
            email_to: String::new(),
            threshold: 5.0,
            alert_after_minutes: 5,
            notify_recovery: true,
        }
    }
}

#[derive(Default)]
struct LowRateState {
    below_since: Option<Instant>,
    alerted: bool,
}

struct DecodeRateNotifications {
    host: Host,
    config: Config,
    /// Per-site low-rate timer and whether its alert has already been sent.
    low_rate: HashMap<String, LowRateState>,
}

impl Plugin for DecodeRateNotifications {
    type Config = Config;
    type SystemConfig = NoConfig;

    fn manifest() -> Manifest {
        Manifest {
            name: "Decode Rate Notifications".into(),
            description: "Sends decode-rate alerts through Pushover, webhooks, or email".into(),
            subscribe: vec![topic::STATUS.into()],
            ..trunk_recorder_plugin::manifest!()
        }
    }

    fn start(host: Host, setup: Setup<Config, NoConfig>) -> Result<Self, String> {
        let config = setup.config;
        let has_app_token = !config.app_token.trim().is_empty();
        let has_user_key = !config.user_key.trim().is_empty();
        if has_app_token != has_user_key {
            return Err("Pushover requires both app_token and user_key".into());
        }

        let webhook_url = config.webhook_url.trim();
        if !webhook_url.is_empty() && !(webhook_url.starts_with("https://") || webhook_url.starts_with("http://")) {
            return Err("webhook_url must start with http:// or https://".into());
        }

        let email_configured =
            [config.smtp_host.trim(), config.smtp_username.trim(), config.smtp_password.trim(), config.email_from.trim(), config.email_to.trim()]
                .iter()
                .any(|value| !value.is_empty());
        if email_configured {
            if config.smtp_host.trim().is_empty() || config.email_from.trim().is_empty() || config.email_to.trim().is_empty() {
                return Err("email requires smtp_host, email_from, and email_to".into());
            }
            if config.smtp_port == 0 {
                return Err("smtp_port must be greater than zero".into());
            }
            if !["starttls", "tls", "none"].contains(&config.smtp_security.trim().to_ascii_lowercase().as_str()) {
                return Err("smtp_security must be starttls, tls, or none".into());
            }
            if config.smtp_username.trim().is_empty() != config.smtp_password.trim().is_empty() {
                return Err("smtp_username and smtp_password must both be set or both be blank".into());
            }
            config.email_from.parse::<Mailbox>().map_err(|error| format!("email_from is invalid: {error}"))?;
            for recipient in config.email_to.split(',').map(str::trim).filter(|value| !value.is_empty()) {
                recipient.parse::<Mailbox>().map_err(|error| format!("email_to contains an invalid address: {error}"))?;
            }
            if config.email_to.split(',').all(|value| value.trim().is_empty()) {
                return Err("email_to must contain at least one address".into());
            }
        }

        if !has_app_token && webhook_url.is_empty() && !email_configured {
            return Err("configure at least one notification route: Pushover, webhook, or email".into());
        }
        if !config.threshold.is_finite() || config.threshold <= 0.0 {
            return Err("decode rate threshold must be a positive number".into());
        }

        Ok(Self { host, config, low_rate: HashMap::new() })
    }

    fn status(&mut self, status: Status) {
        for system in status.systems {
            let site = if system.site_name.is_empty() { system.short_name.clone() } else { format!("{} / {}", system.short_name, system.site_name) };
            let now_below = system.decode_rate < self.config.threshold;
            let state = self.low_rate.entry(site.clone()).or_default();

            if now_below {
                let below_since = *state.below_since.get_or_insert_with(Instant::now);
                if !state.alerted && below_since.elapsed() >= Duration::from_secs(self.config.alert_after_minutes.saturating_mul(60)) {
                    state.alerted = true;
                    let message = format!("{site} decode rate is {:.1} messages/s (below {:.1})", system.decode_rate, self.config.threshold);
                    send_notifications(
                        self.host.clone(),
                        self.config.clone(),
                        "low_rate",
                        site.clone(),
                        system.decode_rate,
                        self.config.threshold,
                        "Decode rate is low",
                        message,
                    );
                }
            } else {
                let should_notify_recovery = state.alerted && self.config.notify_recovery;
                *state = LowRateState::default();
                if should_notify_recovery {
                    let message = format!("{site} decode rate recovered to {:.1} messages/s", system.decode_rate);
                    send_notifications(
                        self.host.clone(),
                        self.config.clone(),
                        "recovery",
                        site,
                        system.decode_rate,
                        self.config.threshold,
                        "Decode rate recovered",
                        message,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn send_notifications(host: Host, config: Config, event: &'static str, site: String, decode_rate: f64, threshold: f64, title: &'static str, message: String) {
    if !config.app_token.trim().is_empty() {
        send_pushover(host.clone(), config.app_token.clone(), config.user_key.clone(), title.into(), message.clone());
    }

    if !config.webhook_url.trim().is_empty() {
        let host = host.clone();
        let url = config.webhook_url.clone();
        let payload = serde_json::json!({
            "event": event,
            "site": site,
            "decode_rate": decode_rate,
            "threshold": threshold,
            "title": title,
            "message": message,
        });
        thread::spawn(move || match ureq::post(&url).send_json(payload) {
            Ok(_) => host.info("Webhook notification sent"),
            Err(_) => host.warn("Couldn't send webhook notification"),
        });
    }

    if !config.smtp_host.trim().is_empty() {
        send_email(
            host,
            SmtpConfig {
                host: config.smtp_host,
                port: config.smtp_port,
                security: config.smtp_security.to_ascii_lowercase(),
                username: config.smtp_username,
                password: config.smtp_password,
                from: config.email_from,
                to: config.email_to,
            },
            title.into(),
            message,
        );
    }
}

fn send_pushover(host: Host, token: String, user: String, title: String, message: String) {
    thread::spawn(move || {
        let response =
            ureq::post(PUSHOVER_URL).send_form([("token", token.as_str()), ("user", user.as_str()), ("title", title.as_str()), ("message", message.as_str())]);

        match response {
            Ok(mut response) => match response.body_mut().read_to_string() {
                Ok(body) => match serde_json::from_str::<PushoverResponse>(&body) {
                    Ok(result) if result.status == 1 => host.info("Pushover alert sent"),
                    Ok(result) => host.warn(format!("Pushover rejected an alert: {}", result.errors.join(", "))),
                    Err(error) => host.warn(format!("Couldn't read Pushover response: {error}")),
                },
                Err(error) => host.warn(format!("Couldn't read Pushover response: {error}")),
            },
            Err(error) => host.warn(format!("Couldn't send Pushover alert: {error}")),
        }
    });
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    security: String,
    username: String,
    password: String,
    from: String,
    to: String,
}

fn send_email(host: Host, config: SmtpConfig, title: String, message: String) {
    thread::spawn(move || match send_smtp_email(&config, &title, &message) {
        Ok(()) => host.info("Email notification sent"),
        Err(error) => host.warn(format!("Couldn't send email notification: {error}")),
    });
}

fn send_smtp_email(config: &SmtpConfig, title: &str, message: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut email_builder = Message::builder().from(config.from.parse::<Mailbox>()?).subject(title);
    for recipient in config.to.split(',').map(str::trim).filter(|value| !value.is_empty()) {
        email_builder = email_builder.to(recipient.parse::<Mailbox>()?);
    }
    let email = email_builder.body(message.to_owned())?;

    let mut transport_builder = match config.security.as_str() {
        "tls" => SmtpTransport::relay(&config.host)?,
        "starttls" => SmtpTransport::starttls_relay(&config.host)?,
        "none" => SmtpTransport::builder_dangerous(&config.host),
        _ => return Err("smtp_security must be starttls, tls, or none".into()),
    }
    .port(config.port);

    if !config.username.is_empty() {
        transport_builder = transport_builder.credentials(Credentials::new(config.username.clone(), config.password.clone()));
    }

    transport_builder.build().send(&email)?;
    Ok(())
}

#[derive(Deserialize)]
struct PushoverResponse {
    status: u8,
    #[serde(default)]
    errors: Vec<String>,
}

fn main() {
    trunk_recorder_plugin::run::<DecodeRateNotifications>();
}
