//! Legacy stock provenance shared by library loading and chooser rendering.
use super::*;

#[cfg(test)]
thread_local! {
    pub(super) static LEGACY_NAME_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn is_known_migrated_stock_pictogram(icon: &SavedPictogram) -> bool {
    match icon.origin {
        PictogramOrigin::User => return false,
        PictogramOrigin::Stock => return true,
        PictogramOrigin::Legacy => {}
    }
    use std::collections::HashMap;
    use std::sync::OnceLock;
    type StockRecords = HashMap<String, Vec<[u8; PICTOGRAM_BYTES]>>;
    static RECORDS: OnceLock<StockRecords> = OnceLock::new();
    let records = RECORDS.get_or_init(|| {
        let mut records = StockRecords::new();
        // Include every catalog family, not only the original 38 builtins:
        // historical exports also promoted action/firmware/integration icons.
        let old_firmware = include_bytes!("../assets/legacy-firmware-builtin-icons-c518c3c.bin");
        let pre_thinning = include_bytes!("../assets/legacy-stock-catalog-pre-thinning.bin");
        let two_tier_layers = include_bytes!("../assets/legacy-layer-navigation-afa3e6c.bin");
        // Yurii confirmed these exact old masks were agent-created catalog
        // artwork, not user drawings. Match pixels, never the name alone.
        let agent_layers = include_bytes!("../assets/legacy-layer-navigation-agent-confirmed.bin");
        let firmware_start = 38 + crate::action_icons::ACTION_ICON_COUNT;
        for index in 0..BUILTIN_PICTOGRAM_KEYS.len() {
            for language in crate::i18n::Language::ALL {
                #[cfg(test)]
                LEGACY_NAME_LOOKUPS.with(|count| count.set(count.get() + 1));
                let name = crate::i18n::tr_catalog(language, BUILTIN_PICTOGRAM_KEYS[index]);
                let bitmaps = records.entry(name.to_owned()).or_default();
                if index < 38 {
                    bitmaps.push(legacy_builtin_pictogram_bitmap(index));
                }
                bitmaps.push(builtin_pictogram_bitmap(index));
                // Exact source/artifact-backed older stroke revisions. Names
                // alone are not provenance: unknown/edited pixels stay visible.
                let offset = index * PICTOGRAM_BYTES;
                bitmaps.push(
                    pre_thinning[offset..offset + PICTOGRAM_BYTES]
                        .try_into()
                        .unwrap(),
                );
                if (firmware_start + 20..firmware_start + 22).contains(&index) {
                    let offset = (index - firmware_start - 20) * PICTOGRAM_BYTES;
                    bitmaps.push(
                        two_tier_layers[offset..offset + PICTOGRAM_BYTES]
                            .try_into()
                            .unwrap(),
                    );
                    bitmaps.push(
                        agent_layers[offset..offset + PICTOGRAM_BYTES]
                            .try_into()
                            .unwrap(),
                    );
                }
                if (firmware_start
                    ..firmware_start + crate::firmware_builtin_icons::FIRMWARE_ICON_COUNT)
                    .contains(&index)
                {
                    let offset = (index - firmware_start) * PICTOGRAM_BYTES;
                    bitmaps.push(
                        old_firmware[offset..offset + PICTOGRAM_BYTES]
                            .try_into()
                            .unwrap(),
                    );
                }
            }
        }
        records
    });
    records.get(icon.name.trim()).is_some_and(|bitmaps| {
        bitmaps
            .iter()
            .any(|bitmap| icon.bitmap.as_slice() == bitmap)
    })
}
