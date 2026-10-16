use super::*;

/// Serialized definition contract only; runtime values stay in ModuleSettingsState.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EntModuleField {
    group: String,
    side: String,
    title: String,
    kind: String,
    bit: u8,
    width: u8,
    min: u16,
    max: u16,
    variants: Vec<String>,
}

pub(super) fn module_setting_valid(setting: &EntPortableSetting) -> bool {
    let Some(fields) = &setting.module else {
        return false;
    };
    let width = match setting.width {
        EntPortableSettingWidth::U8 => 1,
        EntPortableSettingWidth::U16 => 2,
    };
    let limit = if width == 1 { 255 } else { u16::MAX };
    if setting.id != "module"
        || fields.is_empty()
        || !setting.variants.is_empty()
        || setting.value > limit
    {
        return false;
    }
    let mut bits = std::collections::BTreeSet::new();
    for field in fields {
        if field.group.is_empty()
            || field.title.is_empty()
            || !matches!(
                field.side.as_str(),
                "left" | "right" | "auto_layer" | "other"
            )
            || !matches!(field.width, 1 | 2)
            || field.width > width
            || field.min > field.max
            || field.max > limit
        {
            return false;
        }
        match field.kind.as_str() {
            "boolean" => {
                // Preserve the raw packed word, including firmware-reserved bits.
                // Every described bit must be distinct and have the same width.
                if field.bit >= width * 8 || !field.variants.is_empty() || !bits.insert(field.bit) {
                    return false;
                }
            }
            "integer" | "select" => {
                if fields.len() != 1
                    || field.bit != 0
                    || setting.value < field.min
                    || setting.value > field.max
                {
                    return false;
                }
                if field.kind == "select" {
                    if field.variants.is_empty()
                        || field.variants.iter().any(|s| s.is_empty())
                        || field.max as usize >= field.variants.len()
                        || setting.value as usize >= field.variants.len()
                    {
                        return false;
                    }
                    let unique = field
                        .variants
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>();
                    if unique.len() != field.variants.len() {
                        return false;
                    }
                } else if !field.variants.is_empty() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

pub(super) fn module_setting_import_value(
    source: &EntPortableSetting,
    target: &EntPortableSetting,
) -> u16 {
    let Some(fields) = &source.module else {
        return source.value;
    };
    if !fields.iter().all(|field| field.kind == "boolean") {
        return source.value;
    }
    let mask = fields
        .iter()
        .fold(0u16, |mask, field| mask | (1u16 << field.bit));
    (source.value & mask) | (target.value & !mask)
}

impl EntropyApp {
    pub(super) fn entlayout_module_settings_snapshot(&self) -> Vec<EntPortableSetting> {
        if !self.module_settings.supported {
            return Vec::new();
        }
        let mut contracts = std::collections::BTreeMap::<u16, Vec<EntModuleField>>::new();
        for group in &self.module_settings.groups {
            for field in &group.fields {
                let side = match group.kind {
                    ModuleSettingsGroupKind::Left => "left",
                    ModuleSettingsGroupKind::Right => "right",
                    ModuleSettingsGroupKind::AutoLayer => "auto_layer",
                    ModuleSettingsGroupKind::Other => "other",
                };
                let kind = match field.kind {
                    ModuleSettingKind::Boolean => "boolean",
                    ModuleSettingKind::Integer => "integer",
                    ModuleSettingKind::Select => "select",
                };
                contracts
                    .entry(field.qsid)
                    .or_default()
                    .push(EntModuleField {
                        group: group.title.clone(),
                        side: side.to_owned(),
                        title: field.title.clone(),
                        kind: kind.to_owned(),
                        bit: field.bit,
                        width: field.width,
                        min: field.min,
                        max: field.max,
                        variants: field.variants.clone(),
                    });
            }
        }
        contracts
            .into_iter()
            .filter_map(|(qsid, fields)| {
                if !self.supported_qmk_settings.contains(&qsid) {
                    return None;
                }
                let value = *self.module_settings.values.get(&qsid)?;
                let width = match fields.iter().map(|field| field.width).max()? {
                    1 => EntPortableSettingWidth::U8,
                    2 => EntPortableSettingWidth::U16,
                    _ => return None,
                };
                let setting = EntPortableSetting {
                    id: "module".to_owned(),
                    qsid,
                    width,
                    value,
                    variants: Vec::new(),
                    module: Some(fields),
                };
                module_setting_valid(&setting).then_some(setting)
            })
            .collect()
    }
}
