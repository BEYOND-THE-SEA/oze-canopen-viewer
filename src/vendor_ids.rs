use std::{collections::HashMap, sync::OnceLock};

static VENDOR_MAP: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();

fn load_vendor_map() -> HashMap<u32, &'static str> {
    let json = include_str!("../data/canopen_vendor_ids.json");
    let raw: HashMap<String, String> =
        serde_json::from_str(json).expect("invalid data/canopen_vendor_ids.json");

    let mut out: HashMap<u32, &'static str> = HashMap::with_capacity(raw.len());
    for (k, v) in raw {
        let key = k.trim().trim_start_matches("0x");
        let Ok(id) = u32::from_str_radix(key, 16) else {
            continue;
        };
        let company = v.trim();
        if company.is_empty() {
            continue;
        }
        // Keep a stable &'static str without requiring owned Strings everywhere.
        let leaked: &'static str = Box::leak(company.to_string().into_boxed_str());
        out.entry(id).or_insert(leaked);
    }
    out
}

pub fn vendor_company_name(vendor_id: u32) -> Option<&'static str> {
    let map = VENDOR_MAP.get_or_init(load_vendor_map);
    map.get(&vendor_id).copied()
}

