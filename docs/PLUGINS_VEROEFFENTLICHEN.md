# Plugins veröffentlichen (GitHub)

OpenDeckN3 installiert Plugins auf drei Wegen:

| Weg | In der App | Quelle |
| --- | --- | --- |
| **Datei** | Plugins → „Aus Datei …“ oder Datei auf die Plugins-Seite ziehen | `.streamDeckPlugin` / `.zip` |
| **GitHub-Repository** | Plugins → Katalog → `besitzer/repository` eingeben | neuestes Release des Repositorys |
| **Katalog** | Plugins → Katalog → „Installieren“ / „Aktualisieren“ | Einträge aus [`plugins/registry.json`](../plugins/registry.json) |

Bei allen Wegen landet das Plugin in `<config>/plugins/<uuid>.sdPlugin` (Windows: `%APPDATA%\opendeckn3\plugins`) und
startet sofort – ohne Neustart der App. Eine neuere Version ersetzt die alte, Tastenbelegungen bleiben erhalten.

## 1. Repository anlegen

```text
mein-plugin/                         # GitHub-Repository, z. B. Diddlik/opendeckn3-obs
├── de.diddlik.obs.sdPlugin/
│   ├── manifest.json                # "UUID": "de.diddlik.obs", "Version": "1.0.0"
│   ├── plugin.js                    # oder .py / Binary (CodePath, CodePathWin, …)
│   └── imgs/…
└── .github/workflows/release.yml
```

Aufbau und Protokoll: [PLUGIN_API.md](PLUGIN_API.md). Die `Version` im Manifest sollte zum Release-Tag passen
(`v1.0.0` ↔ `"Version": "1.0.0"`) – daran erkennt die App, ob ein Update verfügbar ist (SemVer-Vergleich).

## 2. Release mit Plugin-Datei

Die App nimmt aus dem **neuesten Release** (Vorabversionen nur, wenn es kein reguläres gibt) ein Asset mit der Endung
`.streamDeckPlugin`, `.zip` oder `.n3plugin`. Gibt es mehrere, gewinnt das mit dem eigenen Betriebssystem im Namen
(`…-windows.zip`, `…-linux.zip`, `…-macos.zip`); Dateien für andere Systeme werden ignoriert. Das Archiv enthält den
Ordner `<uuid>.sdPlugin/` (oder `manifest.json` direkt auf oberster Ebene).

Beispiel-Workflow – erzeugt bei jedem Tag `v*` ein Release mit `<uuid>.streamDeckPlugin`:

```yaml
# .github/workflows/release.yml
name: Release
on:
  push:
    tags: ["v*"]
permissions:
  contents: write
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Plugin packen
        run: |
          PLUGIN=de.diddlik.obs
          VERSION="${GITHUB_REF_NAME#v}"
          jq --arg v "$VERSION" '.Version = $v' "$PLUGIN.sdPlugin/manifest.json" > m.json
          mv m.json "$PLUGIN.sdPlugin/manifest.json"
          zip -r "$PLUGIN.streamDeckPlugin" "$PLUGIN.sdPlugin"
      - uses: softprops/action-gh-release@v2
        with:
          files: "*.streamDeckPlugin"
```

Danach lässt sich das Plugin in der App über **Katalog → `besitzer/repository` → Von GitHub installieren** laden.
GitHub liefert für hochgeladene Assets eine SHA-256-Prüfsumme mit; die App prüft den Download dagegen.

## 3. In den Katalog aufnehmen

Damit das Plugin für alle in der Liste erscheint, einen Eintrag in
[`plugins/registry.json`](../plugins/registry.json) per Pull Request ergänzen:

```json
{
  "uuid": "de.diddlik.obs",
  "name": "OBS Studio",
  "description": "Szenen wechseln, Aufnahme starten, Mikro stumm",
  "author": "Diddlik",
  "repo": "Diddlik/opendeckn3-obs",
  "category": "Streaming"
}
```

| Feld | Pflicht | Bedeutung |
| --- | --- | --- |
| `uuid` | ja | wie im Manifest – verbindet Katalog und installiertes Plugin |
| `name`, `repo` | ja | Anzeigename, GitHub `besitzer/name` |
| `description`, `author`, `category`, `homepage` | nein | Anzeige |
| `asset` | nein | fester Asset-Name, falls ein Release mehrere Archive enthält |

Die App lädt den Katalog von `main` (`raw.githubusercontent.com`) und fragt je Eintrag die Releases bei GitHub ab
(ohne Anmeldung: 60 Abfragen/Stunde, Ergebnisse werden 10 min zwischengespeichert). Eigene oder zusätzliche Kataloge:
`opendeckn3d --plugin-registry <URL|Datei>` (mehrfach möglich).

## Sicherheit

Plugins sind normale Programme und laufen mit den Rechten des Benutzers. Die App lädt nur von
`github.com/<repo>/releases/download/`, prüft vorhandene Prüfsummen, entpackt nur innerhalb des Plugin-Ordners und
fragt vor der Installation aus einem beliebigen Repository nach. Einträge im Katalog werden per Pull Request geprüft.
