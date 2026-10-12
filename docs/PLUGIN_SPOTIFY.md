# Spotify-Plugin

Steuert Spotify vom Deck aus: Titel gefällt mir, zu einer Playlist hinzufügen, Zufallswiedergabe und Wiederholen
umschalten und die Wiedergabe auf ein anderes Gerät verschieben. Play, Pause und Cover liefert das
[Medien-Plugin](PLUGIN_MEDIA.md), das auch ohne Anmeldung funktioniert. Quellcode:
[`plugins/spotify`](../plugins/spotify). Windows und Linux.

## Einrichten

Das Plugin braucht eine eigene Spotify-Anwendung, erkennbar an der **Client-ID**. Ein Client-Secret ist nicht nötig und
wird nicht mitgeliefert (PKCE).

### Einschränkungen seit Februar 2026

Spotify hat die Web-API für Anwendungen im Entwicklermodus eingeschränkt:

- Der Besitzer der Anwendung braucht ein **Spotify Premium**-Konto.
- Im Entwicklermodus sind höchstens **5 Nutzer** eingetragen. Jeder davon muss im Dashboard unter **User Management**
  mit Namen und E-Mail-Adresse eingetragen werden.
- Steuerbefehle (Wiedergabe verschieben, Zufall, Wiederholen) brauchen bei jedem Nutzer **Premium**.
- Spotify begrenzt die Anfragen pro Konto (Kontingent). Ist es aufgebraucht, zeigt der Status „Spotify-Kontingent
  erschöpft“.

### 1. Anwendung bei Spotify registrieren

1. Auf [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard) mit dem Spotify-Konto anmelden, dem
   die Anwendung gehören soll (mit Premium).
2. **Create app** klicken und die Felder ausfüllen:
   - **App name:** frei wählbar, z. B. `OpenDeckN3 Deck`.
   - **Redirect URI:** genau `http://127.0.0.1:57140/callback` eintragen und mit **Add** übernehmen. Die Adresse muss
     exakt stimmen (ohne Schrägstrich am Ende).
   - **Which API/SDKs are you planning to use?:** **Web API** ankreuzen.
3. Die Nutzungsbedingungen bestätigen und **Save** klicken.
4. Unter **Settings** die **Client ID** kopieren. Ein Client-Secret wird nicht gebraucht.
5. Unter **User Management** das eigene Spotify-Konto mit Namen und E-Mail-Adresse eintragen. Ohne diesen Eintrag
   lehnt Spotify die Anmeldung ab.

### 2. Im Plugin anmelden

1. In OpenDeckN3: **Plugins → Katalog → Spotify → Installieren**.
2. **Plugins → Spotify → Einstellungen**: die Client-ID in das Feld **Client-ID** einfügen.
3. **Anmelden** klicken. Der Browser öffnet die Anmeldeseite von Spotify. Dort anmelden und den Zugriff erlauben.
4. Das Plugin nimmt die Antwort auf `127.0.0.1` Port `57140` entgegen. Die Seite zeigt danach „Anmeldung erfolgreich“,
   und der Status im Plugin zeigt „Verbunden als …“. Die Anmeldung bleibt gespeichert und wird automatisch erneuert.

Der Anmeldevorgang wartet höchstens 5 Minuten. Läuft ein anderer Vorgang auf Port `57140`, schlägt die Anmeldung mit
einer Meldung zum belegten Port fehl.

Mit **Abmelden** werden die gespeicherten Anmeldedaten gelöscht. Ändert man die Client-ID, meldet sich das Plugin
ebenfalls ab. Zeigt der Status „Bitte neu anmelden“, hat Spotify den Zugriff widerrufen: erneut **Anmelden** klicken.

## Aktionen

| Aktion | Verhalten |
| --- | --- |
| **Gefällt mir** | Speichert den laufenden Titel in der Bibliothek oder entfernt ihn. Die Taste leuchtet, wenn der Titel gespeichert ist. |
| **Zur Playlist hinzufügen** | Fügt den laufenden Titel der gewählten Playlist hinzu. Zur Auswahl stehen nur eigene und gemeinsame Playlists. |
| **Zufall** | Schaltet die Zufallswiedergabe ein oder aus. Die Taste leuchtet, wenn sie an ist. |
| **Wiederholen** | Schaltet um: aus, Titel wiederholen, Liste wiederholen. Das Symbol zeigt den Zustand. |
| **Gerät wechseln** | Verschiebt die Wiedergabe auf das gewählte Gerät. Die Liste zeigt nur Geräte, die Spotify kennt. |

Aktueller Titel und Zustand werden alle 5 Sekunden abgefragt, solange eine Taste der Spotify-Aktionen sichtbar ist.
Spielt gerade nichts, zeigen die Tasten „–“.

## Technik

- Anmeldung: Authorization Code mit PKCE. Der Verifier bleibt im Plugin; Tokens liegen in den globalen Einstellungen
  des Plugins.
- Benötigte Berechtigungen: `user-read-playback-state`, `user-modify-playback-state`, `user-library-read`,
  `user-library-modify`, `playlist-read-private`, `playlist-read-collaborative`, `playlist-modify-private`,
  `playlist-modify-public`.
- Schnittstelle: `https://api.spotify.com/v1`. Bei `429` wartet das Plugin die Zeit aus `Retry-After`, bevor es erneut
  abfragt.
- Die Bibliothek wird über `/me/library` gelesen und geändert (Spotify-URIs `spotify:track:<id>`).
