"""Rewrites the embedded icon library (a subset of Lucide) in ui/index.html.

Usage: npm pack lucide-static@0.460.0, unpack it, then
    python3 tools/make-iconlib.py <unpacked>/package/icons
"""
import json
import re
import sys

ICON_DIR = sys.argv[1]
UI = 'ui/index.html'

# name: German search words
PICK = {
    # Gaming
    'gamepad-2': 'controller spiel gaming', 'joystick': 'spiel arcade', 'swords': 'kampf schwerter',
    'crosshair': 'zielen fadenkreuz', 'target': 'ziel', 'skull': 'totenkopf tod', 'trophy': 'pokal sieg',
    'medal': 'medaille', 'crown': 'krone', 'shield': 'schild schutz', 'bomb': 'bombe', 'flame': 'feuer',
    'zap': 'blitz energie', 'heart': 'herz leben', 'star': 'stern favorit', 'rocket': 'rakete start',
    'ghost': 'geist', 'sparkles': 'funkeln effekt', 'dice-5': 'würfel zufall', 'puzzle': 'puzzle',
    'map': 'karte', 'flag': 'flagge', 'swords': 'kampf schwerter', 'axe': 'axt', 'gem': 'edelstein',
    # Streaming / OBS
    'video': 'kamera video webcam', 'video-off': 'kamera aus', 'camera': 'foto kamera', 'mic': 'mikrofon',
    'mic-off': 'mikrofon stumm', 'headphones': 'kopfhörer', 'headset': 'headset', 'radio': 'live sendung',
    'cast': 'stream übertragen', 'tv': 'fernseher', 'monitor': 'bildschirm', 'monitor-off': 'bildschirm aus',
    'clapperboard': 'szene klappe', 'film': 'film video', 'image': 'bild', 'scissors': 'schnitt clip',
    'circle-dot': 'aufnahme rec', 'square': 'stopp', 'pause': 'pause', 'play': 'abspielen start',
    'skip-forward': 'weiter nächster', 'skip-back': 'zurück voriger', 'repeat': 'wiederholen', 'shuffle': 'zufall',
    'volume-2': 'lautstärke laut', 'volume-1': 'lautstärke leise', 'volume-x': 'stumm ton aus',
    'music': 'musik', 'list-music': 'playlist musik', 'screen-share': 'bildschirm teilen',
    'layers': 'ebenen quellen', 'sliders-horizontal': 'regler filter mixer', 'eye': 'sichtbar zeigen',
    'eye-off': 'versteckt ausblenden', 'save': 'speichern', 'trash-2': 'löschen papierkorb',
    'folder': 'ordner', 'folder-open': 'ordner öffnen', 'download': 'herunterladen', 'upload': 'hochladen',
    'share-2': 'teilen', 'bookmark': 'lesezeichen merken', 'highlighter': 'highlight markieren',
    'party-popper': 'gg feier', 'thumbs-up': 'daumen hoch gut', 'thumbs-down': 'daumen runter',
    'smile': 'lächeln', 'laugh': 'lachen', 'frown': 'traurig', 'angry': 'wütend',
    # Communication
    'message-circle': 'chat nachricht', 'messages-square': 'chat kanal', 'users': 'gruppe team',
    'user': 'person', 'user-plus': 'einladen', 'phone': 'telefon anruf', 'phone-off': 'auflegen',
    'bell': 'benachrichtigung glocke', 'bell-off': 'nicht stören', 'at-sign': 'erwähnung', 'mail': 'e-mail post',
    'globe': 'web internet', 'link': 'link', 'wifi': 'wlan netzwerk', 'bluetooth': 'bluetooth',
    # Home
    'house': 'zuhause home', 'lightbulb': 'licht lampe', 'lamp': 'lampe', 'plug': 'steckdose strom',
    'thermometer': 'temperatur heizung', 'fan': 'lüfter ventilator', 'lock': 'schloss abschließen',
    'lock-open': 'aufschließen', 'key': 'schlüssel', 'door-open': 'tür', 'blinds': 'rollo jalousie',
    'sun': 'sonne hell', 'moon': 'mond nacht dunkel', 'cloud': 'wolke wetter', 'droplet': 'wasser tropfen',
    # System / Office
    'settings': 'einstellungen zahnrad', 'power': 'ausschalten an aus', 'refresh-cw': 'aktualisieren neu laden',
    'rotate-ccw': 'rückgängig zurück', 'clock': 'uhr zeit', 'timer': 'timer stoppuhr', 'alarm-clock': 'wecker',
    'calendar': 'kalender termin', 'coffee': 'kaffee pause', 'pizza': 'pizza essen', 'beer': 'bier',
    'terminal': 'konsole befehl', 'code': 'code programmieren', 'keyboard': 'tastatur', 'mouse': 'maus',
    'cpu': 'prozessor leistung', 'hard-drive': 'festplatte', 'command': 'befehl', 'app-window': 'fenster app',
    'calculator': 'rechner', 'file-text': 'datei dokument', 'clipboard': 'zwischenablage', 'notebook-pen': 'notiz',
    'search': 'suche', 'shopping-cart': 'einkauf warenkorb', 'dollar-sign': 'geld', 'car': 'auto',
    'plane': 'flugzeug', 'dog': 'hund', 'cat': 'katze', 'leaf': 'blatt natur',
    # Arrows and symbols
    'arrow-left': 'pfeil links zurück', 'arrow-right': 'pfeil rechts weiter', 'arrow-up': 'pfeil hoch',
    'arrow-down': 'pfeil runter', 'chevrons-right': 'weiter vorspulen', 'chevrons-left': 'zurückspulen',
    'plus': 'plus hinzufügen', 'minus': 'minus', 'check': 'haken ok fertig', 'x': 'kreuz schließen',
    'circle-alert': 'warnung achtung', 'info': 'info', 'hash': 'raute kanal', 'circle': 'kreis punkt',
    'triangle': 'dreieck', 'hexagon': 'sechseck',
}

lib = []
missing = []
for name, words in PICK.items():
    try:
        svg = open(f'{ICON_DIR}/{name}.svg', encoding='utf-8').read()
    except FileNotFoundError:
        missing.append(name)
        continue
    inner = re.search(r'<svg[^>]*>(.*)</svg>', svg, re.S).group(1)
    inner = re.sub(r'\s+', ' ', inner).replace(' />', '/>').replace('> <', '><').strip()
    lib.append([name, words, inner])

if missing:
    sys.exit('missing in this Lucide version: ' + ', '.join(missing))
html = open(UI, encoding='utf-8').read()
line = 'const ICONLIB = ' + json.dumps(lib, ensure_ascii=False, separators=(',', ':')) + ';'
html, n = re.subn(r'^const ICONLIB = .*;$', lambda _: line, html, count=1, flags=re.M)
if n != 1:
    sys.exit('const ICONLIB line not found in ' + UI)
open(UI, 'w', encoding='utf-8', newline='').write(html)
print(len(lib), 'icons written to', UI)
