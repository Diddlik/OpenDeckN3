# 12 – GitHub

- UUID `de.opendeckn3.github`, Ordner `plugins/github`, Win + Linux. HTTP mit `reqwest` (+ `json`).
- Zugang: Fine-grained Personal Access Token (nur lesend) als `password` in den globalen Einstellungen. Ohne Token
  funktionieren öffentliche Repos (60 Abfragen/h).
- Header: `Authorization: Bearer …`, `Accept: application/vnd.github+json`, `X-GitHub-Api-Version: 2022-11-28`,
  `User-Agent: OpenDeckN3`.
- Abfragen alle 60 s, nur für sichtbare Tasten; gleiche Anfragen mehrerer Tasten zusammenlegen.

## Aktionen

| UUID-Suffix | Name | Aufruf | Anzeige | Taste öffnet |
| --- | --- | --- | --- | --- |
| `ci` | CI-Status | `GET /repos/{repo}/actions/runs?branch={branch}&per_page=1` (optional `workflow`) | Zustand 0 = erfolgreich, 1 = läuft, 2 = fehlgeschlagen; Titel Workflow-Name | `html_url` des Laufs |
| `prs` | Pull Requests | `GET /search/issues?q=is:pr+is:open+…` | Anzahl als Titel | Suche auf github.com |
| `notifications` | Benachrichtigungen | `GET /notifications` | Anzahl, Zustand 1 wenn > 0 | `https://github.com/notifications` |

`prs`-Filter (`filter`): `review-requested:@me`, `author:@me`, `repo:{repo}`.
`ci` braucht drei Bilder (`States`) in `tools/make-plugin-icons.cjs`.

## Tests

- Lauf-Zustand aus `status`/`conclusion` ableiten (`queued`, `in_progress`, `success`, `failure`, `cancelled`).
- Suchanfrage für `prs` bauen (URL-Kodierung).
