# 11 – Timer / Pomodoro

- UUID `de.opendeckn3.timer`, Ordner `plugins/timer`, Win + Linux. Nur SDK, `tokio`, `serde_json`, `anyhow`.
- Laufende Timer leben pro Kontext im Plugin; Startzeit in den Instanz-Einstellungen (`setSettings`) speichern, damit
  ein Timer Seiten-/Profilwechsel übersteht.

## Aktionen

| UUID-Suffix | Name | Controller | Verhalten |
| --- | --- | --- | --- |
| `countdown` | Countdown | Keypad, Encoder | Taste/Drücken: Start/Pause. Drehen im Stillstand: Minuten ±1 (`minutes`, Standard 5). Langer Druck (≥ 0,6 s zwischen `keyDown`/`keyUp`): zurücksetzen. Titel `04:59`; Ende: Zustand 1, `showAlert`, Titel blinkt 10 s |
| `pomodoro` | Pomodoro | Keypad | `work` 25, `short` 5, `long` 15, `rounds` 4. Taste: Start/Pause, langer Druck: zurücksetzen. Titel zeigt Phase und Zeit (`Fokus 24:10`); Zustand 0 = Fokus, 1 = Pause |
| `stopwatch` | Stoppuhr | Keypad | Start/Pause, langer Druck: zurücksetzen; Titel `1:02:03` ab einer Stunde |

Ein gemeinsamer Takt (1 s) aktualisiert alle laufenden Timer.

## Tests

Logik als reine Funktionen mit übergebener Zeit (`Instant` nicht direkt lesen):

- Countdown läuft ab, Pause hält die Restzeit.
- Pomodoro-Reihenfolge: Fokus, kurz, Fokus, …, nach `rounds` Fokus-Phasen lang.
- Formatierung `mm:ss` / `h:mm:ss`.
