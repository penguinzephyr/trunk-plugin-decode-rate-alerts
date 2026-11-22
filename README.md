# Decode Rate Notifications

A [Trunk Recorder Pro](https://github.com/TrunkRecorder/trunk-recorder-pro) plugin that sends notifications when a system's decode rate stays below a configured limit. Notifications can be delivered through Pushover, a generic HTTP webhook, custom SMTP email, or any combination of those routes.

The plugin listens to periodic `status` events and tracks each system site independently. It starts a timer when a site's decode rate falls below the threshold and sends an alert only after the configured delay has elapsed. If the rate recovers first, the timer resets. After an alert, the plugin waits for the rate to return to the threshold or higher before it can send another low-rate alert. Recovery notifications are enabled by default. Network requests run in background threads so they do not hold up status-event processing.

## Requirements

- Trunk Recorder Pro with local plugins enabled
- At least one configured notification route
- Rust and Cargo, if building from source

## Configuration

Set these fields in the plugin's settings in Trunk Recorder Pro. Leave a route's fields blank to disable that route.

| Setting | Default | Description |
|---|---:|---|
| `app_token` | blank | Pushover application/API token |
| `user_key` | blank | Pushover user or group key; required with `app_token` |
| `webhook_url` | blank | HTTP or HTTPS endpoint that accepts JSON POST requests |
| `smtp_host` | blank | SMTP server hostname; configuring it enables email delivery |
| `smtp_port` | `587` | SMTP server port; commonly `587` for STARTTLS or `465` for TLS |
| `smtp_security` | `starttls` | SMTP security mode: `starttls`, `tls`, or `none` |
| `smtp_username` | blank | SMTP login username; configure with `smtp_password` when authentication is required |
| `smtp_password` | blank | SMTP login password; configure with `smtp_username` when authentication is required |
| `email_from` | blank | Sender email address; required when SMTP is configured |
| `email_to` | blank | One or more comma-separated recipient addresses; required when SMTP is configured |
| `threshold` | `5.0` | Alert when decode rate falls below this many messages per second; must be positive and finite |
| `alert_after_minutes` | `5` | Minutes a site must remain below the threshold before an alert is sent; `0` sends on the first low status event |
| `notify_recovery` | `true` | Send a notification when the decode rate returns to the threshold or higher |

At least one route must be configured. Pushover needs both `app_token` and `user_key`. Email needs `smtp_host`, `email_from`, and `email_to`. SMTP username and password must either both be set or both be blank.

Example configuration using a webhook and SMTP email:

```json
{
  "webhook_url": "https://notifications.example.net/trunk-recorder",
  "smtp_host": "smtp.example.net",
  "smtp_port": 587,
  "smtp_security": "starttls",
  "smtp_username": "alerts@example.net",
  "smtp_password": "your-smtp-password",
  "email_from": "alerts@example.net",
  "email_to": "operator@example.net, backup@example.net",
  "threshold": 5.0,
  "alert_after_minutes": 5,
  "notify_recovery": true
}
```

For production email, use `starttls` or `tls`. The `none` mode sends email without transport encryption and should only be used with a trusted local mail relay.

## Webhook payload

The plugin sends an HTTP `POST` request with `Content-Type: application/json`. The JSON object contains:

```json
{
  "event": "low_rate",
  "site": "Site Alpha",
  "decode_rate": 3.2,
  "threshold": 5.0,
  "title": "Decode rate is low",
  "message": "Site Alpha decode rate is 3.2 messages/s (below 5.0)"
}
```

`event` is `low_rate` for an alert and `recovery` for a recovery notification. Webhook delivery succeeds for a successful HTTP response; failures are reported in the plugin host's warning log.

## Notification behavior

A site's low-rate timer begins when a status event first reports a rate below the threshold. The alert is sent on the first subsequent status event after the configured delay has elapsed, provided the rate stayed low. If the rate recovers before then, the timer resets. A recovery notification is sent only if a low-rate alert was already sent. Site state is kept in memory for the lifetime of the plugin process.

Pushover rejections and email or network failures are reported through the plugin host's warning log. Successful deliveries are reported through its info log.

## Build

Build the release executable with:

```sh
cargo build --release
```

The executable is written to `target/release/pushover-notifications` (or `pushover-notifications.exe` on Windows). The Trunk Recorder plugin release workflow builds platform packages when a version tag such as `v0.1.0` is pushed.

## License

MIT. See [LICENSE](LICENSE).
