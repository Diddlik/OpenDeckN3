//! Simulated keyboard input on a dedicated thread.
//!
//! `enigo` talks to the OS input APIs (SendInput on Windows, X11 on Linux) and
//! is not `Send` everywhere, so it lives on its own thread and receives jobs.

use std::sync::mpsc;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use tokio::sync::oneshot;

use super::shortcut::Chord;

#[derive(Debug)]
pub enum InputJob {
    Chords(Vec<Chord>),
    Text(String),
    /// Taps a key `n` times (volume steps, media keys).
    Tap(Key, u32),
}

type Request = (InputJob, oneshot::Sender<Result<(), String>>);

#[derive(Clone)]
pub struct InputHandle {
    jobs: mpsc::Sender<Request>,
}

impl InputHandle {
    pub fn spawn() -> Self {
        let (jobs, rx) = mpsc::channel::<Request>();
        std::thread::Builder::new()
            .name("opendeckn3-input".into())
            .spawn(move || {
                // Created lazily: fails without a desktop session (e.g. CI).
                let mut enigo: Option<Enigo> = None;
                for (job, reply) in rx {
                    let result = (|| {
                        if enigo.is_none() {
                            enigo =
                                Some(Enigo::new(&Settings::default()).map_err(|e| {
                                    format!("Eingabesimulation nicht verfügbar: {e}")
                                })?);
                        }
                        let enigo = enigo.as_mut().expect("initialised above");
                        run(enigo, job).map_err(|e| format!("Eingabe fehlgeschlagen: {e}"))
                    })();
                    reply.send(result).ok();
                }
            })
            .expect("cannot spawn input thread");
        Self { jobs }
    }

    pub async fn run(&self, job: InputJob) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.jobs
            .send((job, tx))
            .map_err(|_| "Eingabe-Thread beendet".to_owned())?;
        rx.await.map_err(|_| "Eingabe-Thread beendet".to_owned())?
    }
}

fn run(enigo: &mut Enigo, job: InputJob) -> Result<(), enigo::InputError> {
    match job {
        InputJob::Chords(chords) => {
            for chord in chords {
                for m in &chord.modifiers {
                    enigo.key(*m, Direction::Press)?;
                }
                let result = enigo.key(chord.key, Direction::Click);
                // Always release modifiers, even if the key failed.
                for m in chord.modifiers.iter().rev() {
                    enigo.key(*m, Direction::Release)?;
                }
                result?;
            }
        }
        InputJob::Text(text) => enigo.text(&text)?,
        InputJob::Tap(key, n) => {
            for _ in 0..n {
                enigo.key(key, Direction::Click)?;
            }
        }
    }
    Ok(())
}
