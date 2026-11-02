//! PulseAudio/PipeWire sink inputs (one per playing stream) through `pactl`.

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{Change, Session, Snapshot, same_app, step_volume};

/// Raw pactl volume that means 100 %.
const PA_VOLUME_NORM: f64 = 65536.0;

struct SinkInput {
    index: u64,
    session: Session,
}

pub async fn snapshot() -> Result<Snapshot> {
    let sessions = sink_inputs()
        .await?
        .into_iter()
        .map(|input| input.session)
        .collect();
    Ok(Snapshot {
        sessions,
        // Linux has no equivalent of the foreground window in pactl.
        foreground: None,
    })
}

pub async fn change(app: String, change: Change) -> Result<()> {
    let inputs = sink_inputs().await?;
    let matching: Vec<&SinkInput> = inputs
        .iter()
        .filter(|input| same_app(&input.session.app, &app))
        .collect();
    let Some(first) = matching.first() else {
        return Ok(());
    };
    match change {
        Change::VolumeStep(delta) => {
            for input in &matching {
                let level = step_volume(input.session.volume, delta);
                let percent = format!("{}%", (level * 100.0).round());
                pactl(&["set-sink-input-volume", &input.index.to_string(), &percent]).await?;
            }
        }
        Change::ToggleMute => {
            let muted = if first.session.muted { "0" } else { "1" };
            for input in &matching {
                pactl(&["set-sink-input-mute", &input.index.to_string(), muted]).await?;
            }
        }
    }
    Ok(())
}

async fn sink_inputs() -> Result<Vec<SinkInput>> {
    let json = pactl(&["-f", "json", "list", "sink-inputs"]).await?;
    parse_sink_inputs(&json)
}

async fn pactl(args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("pactl")
        .args(args)
        .output()
        .await
        .context("pactl nicht gefunden")?;
    ensure!(
        output.status.success(),
        "pactl {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?)
}

/// Parses `pactl -f json list sink-inputs`. The volume is the first channel;
/// the app is named by its process binary, or by its application name.
fn parse_sink_inputs(json: &str) -> Result<Vec<SinkInput>> {
    let inputs: Value = serde_json::from_str(json).context("pactl-Ausgabe ungültig")?;
    let inputs = inputs.as_array().context("pactl-Ausgabe ist keine Liste")?;
    Ok(inputs
        .iter()
        .filter_map(|input| {
            let props = &input["properties"];
            let app = props["application.process.binary"]
                .as_str()
                .or_else(|| props["application.name"].as_str())?
                .to_owned();
            let channel = input["volume"].as_object()?.values().next()?;
            Some(SinkInput {
                index: input["index"].as_u64()?,
                session: Session {
                    app,
                    volume: (channel["value"].as_f64()? / PA_VOLUME_NORM) as f32,
                    muted: input["mute"].as_bool().unwrap_or(false),
                },
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {
        "index": 42,
        "driver": "protocol-native.c",
        "sink": 0,
        "mute": false,
        "volume": {
          "front-left": {"value": 32768, "value_percent": "50%", "db": "-18.06 dB"},
          "front-right": {"value": 32768, "value_percent": "50%", "db": "-18.06 dB"}
        },
        "properties": {
          "application.name": "Chromium",
          "application.process.binary": "chromium",
          "media.name": "Video"
        }
      },
      {
        "index": 57,
        "driver": "protocol-native.c",
        "sink": 0,
        "mute": true,
        "volume": {
          "mono": {"value": 65536, "value_percent": "100%", "db": "0.00 dB"}
        },
        "properties": {
          "application.name": "WEBRTC VoiP"
        }
      }
    ]"#;

    #[test]
    fn parses_sink_inputs() {
        let inputs = parse_sink_inputs(SAMPLE).unwrap();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].index, 42);
        assert_eq!(inputs[0].session.app, "chromium");
        assert_eq!(inputs[0].session.volume, 0.5);
        assert!(!inputs[0].session.muted);
        assert_eq!(inputs[1].session.app, "WEBRTC VoiP");
        assert!(inputs[1].session.muted);
    }

    #[test]
    fn rejects_non_list_output() {
        assert!(parse_sink_inputs("{}").is_err());
        assert!(parse_sink_inputs("kein JSON").is_err());
    }
}
