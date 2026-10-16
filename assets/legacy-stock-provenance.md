# Historical stock compatibility snapshots

These files are lookup-only migration inputs. They do not replace current
picker/editor/firmware artwork, and contain no user library records.

- `legacy-stock-catalog-pre-thinning.bin`: 188 consecutive 154-byte MSB-first
  masks in `BUILTIN_PICTOGRAM_KEYS` order. SHA-256
  `ad725f87fa5e2f14e91bd9a3b846f7aa90c235d4c497454b15bf28b41c4ee5c0`.
  - Entries 0–37: compiled actual `Icon35` / native stock renderer from Entropy
    `8d2814e` (before stock stroke refinement).
  - Entries 38–154: Lucide 0.468.0, original SVG stroke=2, 35px CairoSVG raster,
    alpha cutoff=72; same ordered symbols/aliases as `generate_action_icons.py`.
  - Entries 155–174: same original Lucide stroke=2, same firmware symbol list and
    mouse-button emphasis as `generate_builtin_key_icons.py`.
  - Entries 175–176: original three-tier stock layer pair from historical shipped
    `icon-unified-Entropy.AppDir/usr/bin/entropy`; contiguous bytes at offsets
    2756175 and 2756329. The public executable SHA-256 is
    `bd4d7111c835d39fc7b30f28d2ab1769077114f8698a5df85e2b89be5cf6ba3a`.
    Those exact images precede the two-tier and Equal-Corner replacements.
  - Entries 177–187: compiled actual integration renderer from QMK
    `077eff8801:keyboards/ergohaven/macropad/screen_layout.c`.
  - Rebuild with `tools/snapshot_legacy_stock_catalog.py`, supplying the public
    firmware checkout, Lucide tarball and pinned historical executable. This
    tool never reads a user's pictograms.json.
- `legacy-layer-navigation-afa3e6c.bin`: verbatim entries 20/21 from Entropy
  `afa3e6c:assets/firmware-builtin-icons.bin` (two-tier stock layer pair).
  SHA-256 `3e96845666fa97381b5b2999be5ba212c5852f1e9b33836e6e78f2f2aabd4ff3`.
- `legacy-firmware-builtin-icons-c518c3c.bin`: the previously retained delivered
  firmware-stock snapshot; unchanged by this repair.

## Confirmed agent-authored layer pair

`legacy-layer-navigation-agent-confirmed.bin` contains only the two 154-byte
Previous/Next layer masks whose agent (not user) authorship Yurii explicitly
confirmed after fix3. SHA-256:
`4b8d76964515b9b02dfdb06928c6857d167bfe8f0edb69ae18a567b68ad537c4`.
Exact masks were recovered from entries 171/172 of the supplied library; no
other private records are included. They are lookup-only legacy catalog data,
not replacement current artwork. Recognition still matches name and exact
pixels, and explicit `user` origin or pixel edits remain visible.

## Provenance and compatibility policy

`origin` is optional on saved records: absence means legacy/unknown, `user`
means explicitly authored/saved, and `stock` means a catalog snapshot.
New editor saves carry `user`; exports' builtin entries carry `stock`.
The production reader annotates only exact recognized legacy name/mask pairs as
stock, after normalizing legacy 32px masks. Recognition does not depend on RGB.
The saved group honors explicit origin before comparing historical stock masks.

Do not infer stock origin from a name alone, erase records, reset the library,
or classify edited/unknown masks by fuzzy similarity. A legacy intentional exact
copy is indistinguishable from a promoted stock record; all source record bytes
remain stored, and explicit `user` origin can retain that copy. Conversely,
unknown alterations to historical layer masks need authorship confirmation or
an independently verified historical source/artifact before being hidden.

The ignored production reader + full-App::ui replay test accepts a local file
path through `ENTROPY_PICTOGRAM_REPLAY_FILE`; it embeds no private input. Its
all-stock gate defaults to zero visible names. A reviewed list of preserved
unknown records can be passed via `ENTROPY_PICTOGRAM_REPLAY_VISIBLE_NAMES`.
Passing that reviewed-list replay is not proof that every record was stock.
