# Home-Assistant-Plugin

Steuert **Home Assistant** über die REST-API und Webhooks. Quellcode: [`plugins/homeassistant`](../plugins/homeassistant).

## Einrichten

1. **Plugins → Katalog → Home Assistant → Installieren.**
2. Unter **Plugins → Home Assistant → Einstellungen**:
   - **Adresse**, z. B. `http://homeassistant.local:8123` oder `http://192.168.1.10:8123`.
   - **Langlebiges Zugriffstoken**: in Home Assistant *Profil (unten links) → Sicherheit → Langlebige Zugriffstoken →
     Token erstellen*. Für reine Webhooks nicht nötig.
   - **Webhooks**: eine Zeile pro Webhook, `Name = Webhook-ID`.
3. **Verbindung prüfen** – der Status zeigt „Verbunden mit <Zuhause> <Version>“.

## Webhooks

In Home Assistant eine Automation mit dem Auslöser **Webhook** anlegen; die dort angezeigte **Webhook-ID** in die
Liste des Plugins eintragen, z. B.

```text
Licht Wohnzimmer = licht_wohnzimmer
Gute Nacht = gute_nacht
```

Dann eine Taste mit der Aktion **Webhook** belegen und den Webhook aus der Liste wählen (oder „Andere Webhook-ID …“).
Optional werden Daten mitgeschickt (JSON oder Text), in der Automation als `trigger.json` bzw. `trigger.data`
verfügbar. Webhooks laufen ohne Token – die ID ist das Geheimnis. Erfolg/Fehler zeigt die Taste kurz an.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Webhook | Webhook auslösen (POST/PUT/GET, optional Daten) | – | – |
| Dienst ausführen | beliebiger Dienst (Szene, Skript, Licht …), Entität und JSON-Daten optional | – | – |
| Schalter | Entität umschalten (`homeassistant.toggle`) | – | leuchtet, wenn an/offen/spielt; optional Zustand als Text |
| Dimmer | an/aus | Helligkeit (Licht), Lautstärke (Mediaplayer), Lüfter, Rollladen, `input_number` | Prozentwert |
| Zustand anzeigen | aktualisieren | – | Zustand mit Einheit, z. B. `21,5 °C` |

Dienste und Entitäten wählst du aus Listen, die das Plugin live von Home Assistant holt. Zustände werden alle
3 Sekunden abgefragt, solange eine Taste sie anzeigt.

## Technik

REST-API: `GET /api/config`, `GET /api/states`, `GET /api/services`, `POST /api/services/<domain>/<dienst>` mit
`Authorization: Bearer <token>`; Webhooks: `POST /api/webhook/<id>`. Tests ohne Home Assistant:
`cargo test -p opendeckn3-homeassistant` (simulierter Server).
