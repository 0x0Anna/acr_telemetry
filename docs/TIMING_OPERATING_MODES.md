# Timing & RTSS — Betriebsmodi (Soll-Spezifikation)

**Repo:** `acc-stage-timing` (dieser Ordner)  
**Binary:** `acr_timing` (aus diesem Verzeichnis bauen und starten)  
**Referenz-Commit (Baseline):** `0b4cf4f` — lokal, Stand vor Pause-Fix und vollständiger Provider-Δ-Korrektur  
**RTSS-Preset:** `[osd_display] preset = "minimal"` in `acr_timing.toml`

Dieses Dokument ist die **verbindliche Zielbeschreibung** für Implementierung und Reviews.  
Bei Abweichungen im Code: Bug gegen dieses Dokument, nicht „by design“.

---

## 1. Streckenmodell (4 Sektoren, 5 Marker)

| Marker | Bedeutung |
|--------|-----------|
| **Start** | Beginn Sektor 1; Zeitnahme startet |
| **Sektor 1** | Ende Sektor 1, Beginn Sektor 2 |
| **Sektor 2** | Ende Sektor 2, Beginn Sektor 3 |
| **Sektor 3** | Ende Sektor 3, Beginn Sektor 4 |
| **Finish** | Ende Sektor 4; **Zeitnahme stoppt** |

Es gibt **keinen** Marker „Sektor 4“. Sektor 4 ist das Teilstück **Sektor 3 → Finish**.

---

## 2. Begriffe (präzise)

| Begriff | Bedeutung |
|---------|-----------|
| **Referenzzeit** | Vergleichszeit pro Sektor aus `timing_pb` / DB gemäß `[reference_times]` in `acr_timing.toml` (`best_sector`, `best_stage`, …). **Nicht** zwingend „persönliche Bestzeit“ im umgangssprachlichen Sinn — immer **Referenz**. |
| **Sektor-*i*-Zeit (Provider)** | Vom **external timing provider** gelieferte offizielle Zeit für Sektor *i* (1…4), zugeordnet zur **Klammer *i*** in der oberen RTSS-Zeile. |
| **Δ Sektor *i*** | `ProviderSector_i − Referenzzeit_i` (nach Übernahme der Provider-Spielzeit). |
| **Kumuliertes Δ (große mittlere Zeile)** | Laufweite Abweichung gegenüber Referenz über den aktiven Lauf; Verhalten siehe `delta_scope` (Abschnitt 6). |
| **1-Hz-Zeitkorrektur** | Laufende Anpassung der Replik-Spielzeit an den **external timing provider** (`acr_game_clock.jsonl`): z. B. System Δt = 300 ms, Spiel Δt = 250 ms → Replik um 50 ms nachziehen. Läuft **parallel** zu den Sektor-Übernahmen an den Markern. |
| **Sektor-Übernahme (Marker)** | An S1-, S2-, S3-, Finish-Grenze: Sektorzeit = Provider-Wert für diese Klammer; Δ Sektor und **kumuliertes Δ** um die Differenz (Alt-Anzeige − Provider) **mitkorrigieren**. |

---

## 3. RTSS — drei Zeilen (`minimal`)

### 3.1 Vor Track-Lock

- Zweck: nur klären, ob **Daten vom external timing provider** ankommen.
- RTSS (optional): **`Game Data available`**, wenn frische `acr_game_clock.jsonl`-Samples da sind; sonst kein/spärlicher Text.
- **Keine** Sektorzeiten, kein kumuliertes Δ.

### 3.2 Gelockt, Pre-Start (Strecke bekannt, Fahrt noch nicht gestartet)

| Zeile | Inhalt |
|-------|--------|
| **Oben** | Referenzzeiten, für den Nutzer erkennbar als Referenz:  
  `ref: [1:31.45] [1:45.22] [2:01.33] [4:12.50] tot: [9:12.34]`  
  (`tot` = Summe der vier Referenz-Sektorzeiten; Werte aus Referenz-Logik, nicht aus dem aktuellen Lauf.) |
| **Mitte** | Kumuliertes Δ: **`0`** (oder äquivalent neutral, nicht „--“). |
| **Unten** | **`Timer ready`** — frische JSONL, Zeitnahme *kann* anspringen.  
  **`Timing ready`** — keine/keine frische Spieldaten; Zeitnahme *kann trotzdem* fehlschlagen.  

**Wichtig:** `Game Data available` (vor Lock) und `Timer ready` / `Timing ready` (pre-start) sind **verschiedene Botschaften**.

### 3.3 Während des Runs

| Zeile | Inhalt |
|-------|--------|
| **Oben** | Pro Sektor eine Klammer in Reihenfolge 1…4: |
| | • **Fertige Sektoren:** `[Provider-Sektorzeit ±Δ]` (Δ zur **Referenz** dieses Sektors). |
| | • **Aktueller Sektor:** nur **laufende Zeit** (tickt), **kein** Δ bis die Grenze passiert. |
| | • **Keine Doppelklammern** (z. B. nicht `[S1][S1]`): Klammer *k* = nur Sektor *k*. |
| **Mitte** | **Großes kumuliertes Δ** (Schrift z. B. 150 %): siehe Abschnitt 6; Farben Abschnitt 7. |
| **Unten** | Leer während normaler Fahrt (kurze Meldungen nur bei Sonderfällen). |

**An jeder Sektor-Grenze (S1, S2, S3) und am Finish:**

1. Sektorzeit vom **external timing provider** in Klammer *i* (Finish → **Sektor 4** / Klammer 4).
2. Δ Sektor *i* neu berechnen.
3. **Kumuliertes Δ anpassen:** Differenz `(bisher verwendete Sektorzeit − Provider-Sektorzeit)` vom kumulierten Δ **abziehen** (Beispiel: Anzeige 1:30.653, Spiel 1:30.543 → kumuliertes Δ um **0,110 s** reduzieren).
4. Am Finish: dasselbe für Sektor 4; danach Zeitnahme beendet.

Die **1-Hz-Zeitkorrektur** (Replik vs. JSONL) läuft **weiter** zwischen den Markern; die **Sektor-Übernahmen** sind zusätzliche, schärfere Korrekturen an den Grenzen.

### 3.4 Nach Finish

- Obere Zeile: Recap der vier Sektoren (optional Karussell, `sector_recap_sec` in TOML).
- Mitte: finales kumuliertes Δ (farbig).
- Unten: nach Bedarf (noch nicht final spezifiziert).

---

## 4. Was „Cumulative / Modular / GeoJSON-Gates“ **nicht** meint (für Nutzer)

Das sind **interne Namen** für:

- **GeoJSON-Gates:** viele kleine Zwischenpunkte entlang der Strecke (Dateien unter `timing/cumulative_sectors/`), nicht die fünf Hauptmarker Start/S1/S2/S3/Finish.
- **Modular / Presenter:** Programmteil, der diese Gates auswertet und Zwischenzeiten meldet.

**Für die Steuerung des großen kumulierten Δ** zählt nur **`[delta_display] delta_scope`** in `acr_timing.toml` (Abschnitt 6) — nicht die internen Modulnamen.

---

## 5. `delta_scope` — wann setzt sich das kumulierte Δ zurück?

Einstellung in `acr_timing.toml`: `[delta_display] delta_scope = "stage" | "sector" | "subsector"`.

| Wert | Kumuliertes Δ (große Zeile) |
|------|-----------------------------|
| **`stage`** | **Kein Reset** während eines Laufs; summiert über den ganzen Run (Hauptmodus laut Nutzer). Reset nur bei neuem Lauf / Timing-Reset. |
| **`sector`** | **Reset** zu Beginn jedes **Hauptsektors** (nach jedem Marker S1, S2, S3 — d. h. neuer Sektor 2, 3, 4). |
| **`subsector`** | **Reset** nach **jedem** GeoJSON-Gate (Zwischenpunkt), nicht nur an den fünf Hauptmarkern. |

**Unabhängig davon:** An **jedem** Hauptmarker (S1, S2, S3, Finish) werden die **Sektorzeiten vom external timing provider** übernommen und das kumulierte Δ um die Sektor-Korrektur **mitjustiert** (Abschnitt 3.3), auch wenn `delta_scope = stage`.

---

## 6. Sonderfall: Pause / Spiel steht, JSONL lebt noch

**Symptom:** System-/Replik-Zeit und korrigierte Spielzeit laufen auseinander (> **1,0 s**), aber das letzte Provider-Paket ist **nicht älter als 2,0 s** (Daten kommen, `race_time` bewegt sich nicht → Pause).

**Anzeige:**

- Kumuliertes Δ (mittlere Zeile): **`--`**
- (Obere Zeile: nach Implementierung abstimmen; mindestens kein irreführendes Δ.)

**Wiederaufnahme:**

- Erst wieder normal anzeigen, wenn die **korrigierte Spielzeit** **zweimal hintereinander** um weniger als **1,0 s** von der vorherigen Anzeige abweicht (Zeit läuft wieder).

**Status:** Implementiert in `acc-stage-timing` — Replik/Spielzeit-Sync + `--` auf der mittleren Zeile bei Pause.

---

## 7. Farben (großes kumuliertes Δ)

| Δ | Farbe (RTSS) |
|---|----------------|
| Positiv (langsamer) | Rot (`slower_color`, Standard `ff0000`) |
| Negativ (schneller) | Grün (`faster_color`, Standard `00ff00`) |
| \|Δ\| ≤ `neutral_zone_sec` | Neutral / Default (z. B. orange oder ohne Tag) |

---

## 8. Implementierungs-Checkliste (Abnahme im Spiel)

Vor Merge / nach Änderung an Timing/RTSS:

1. Start aus `acc-stage-timing`; Log: `osd_display: preset=Minimal`.
2. Vor Lock: bei JSONL → `Game Data available` (wenn spezifiziert aktiv).
3. Pre-Start: `ref: […] … tot: […]`, Mitte `0`, unten `Timer ready` / `Timing ready`.
4. **Kein** `Afon: ref | cur | delta` im Minimal-Modus.
5. Im Run: Klammer *k* = Provider-Sektor *k*; kein Doppel; laufende Zeit nur im aktiven Sektor.
6. An S1/S2/S3/Finish: Provider übernimmt Sektorzeit; kumuliertes Δ springt um Korrekturdifferenz.
7. Finish: Sektor **4** in Klammer 4; Zeitnahme stoppt.
8. Pause-Sonderfall: `--` laut Abschnitt 6 (wenn implementiert).

---

## 9. Bekannte Abweichungen in `0b4cf4f`

| Thema | Soll (dieses Dokument) | Stand `0b4cf4f` |
|-------|------------------------|-----------------|
| Minimal statt Afon-Zeile | ja | weitgehend |
| `ref:` + `tot:` Pre-Start | ja | teils (ohne `ref:`/`tot:`-Prefix) |
| Provider-Sektor → Klammer *i* | ja | Adopt-Pfad vorhanden, Grenzfälle prüfen |
| Kumuliertes Δ bei Sektor-Korrektur mitziehen | ja | **prüfen / vervollständigen** |
| Pause → `--` | ja | ja (Replik-Sync + `PauseOsdState`) |
| `Game Data available` vor Lock | ja | ggf. noch alte Texte |

---

## 10. Änderungshistorie

| Datum | Änderung |
|-------|----------|
| 2026-05-23 | Erstfassung aus Nutzer-Spezifikation (Grundmodi + Sonderfälle Pause, delta_scope, Referenz-Format). |
