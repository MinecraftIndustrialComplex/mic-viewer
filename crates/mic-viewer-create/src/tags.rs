// SPDX-License-Identifier: MPL-2.0
//! Reading values out of NBT the way Minecraft writes them: a number may be any width, a position
//! an int array or a list of ints, a vector a `{V: [x, y, z]}` compound or a list of doubles.

use std::collections::BTreeMap;

use mic_viewer_surveyor::nbt::Tag;

/// A UUID as Minecraft writes it: four ints.
pub type Uuid = [i32; 4];

pub fn num(t: &Tag) -> Option<f64> {
    Some(match t {
        Tag::Byte(v) => f64::from(*v),
        Tag::Short(v) => f64::from(*v),
        Tag::Int(v) => f64::from(*v),
        Tag::Long(v) => *v as f64,
        Tag::Float(v) => f64::from(*v),
        Tag::Double(v) => *v,
        _ => return None,
    })
}

pub fn int(t: &Tag) -> Option<i64> {
    Some(match t {
        Tag::Byte(v) => i64::from(*v),
        Tag::Short(v) => i64::from(*v),
        Tag::Int(v) => i64::from(*v),
        Tag::Long(v) => *v,
        _ => return None,
    })
}

/// A compound's number at `key`.
pub fn f(t: &Tag, key: &str) -> Option<f64> {
    t.get(key).and_then(num)
}

/// A compound's integer at `key`.
pub fn i(t: &Tag, key: &str) -> Option<i64> {
    t.get(key).and_then(int)
}

/// A compound's flag at `key` (absent: false).
pub fn flag(t: &Tag, key: &str) -> bool {
    i(t, key).is_some_and(|v| v != 0)
}

/// A compound's string at `key`.
pub fn s<'a>(t: &'a Tag, key: &str) -> Option<&'a str> {
    t.get(key).and_then(Tag::as_str)
}

/// Ints from an int array or a list of numbers.
pub fn ints(t: &Tag) -> Option<Vec<i32>> {
    match t {
        Tag::IntArray(v) => Some(v.clone()),
        Tag::List(v) => v.iter().map(|x| int(x).and_then(|n| i32::try_from(n).ok())).collect(),
        _ => None,
    }
}

/// Three ints (a block position).
pub fn pos3(t: &Tag) -> Option<[i32; 3]> {
    let v = ints(t)?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// A UUID (an int array of four).
pub fn uuid(t: &Tag) -> Option<Uuid> {
    let v = ints(t)?;
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// A vector: `{V: [x, y, z]}`, or a list of three numbers.
pub fn vec3(t: &Tag) -> Option<[f64; 3]> {
    let l = match t.get("V") {
        Some(v) => v,
        None => t,
    };
    let v: Vec<f64> = l.items().iter().filter_map(num).collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// A UUID as text (the usual 8-4-4-4-12 hex form).
pub fn uuid_text(u: &Uuid) -> String {
    let b: Vec<u8> = u.iter().flat_map(|x| x.to_be_bytes()).collect();
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Every item stack (`{id, count}`) and fluid stack (`{id, amount}`) under `t`, summed by id into
/// `items` and `fluids`. A stack's own contents (a shulker box's) are not walked into.
pub fn stacks(t: &Tag, items: &mut BTreeMap<String, i64>, fluids: &mut BTreeMap<String, i64>) {
    stacks_at(t, items, fluids, 0);
}

fn stacks_at(t: &Tag, items: &mut BTreeMap<String, i64>, fluids: &mut BTreeMap<String, i64>, depth: usize) {
    if depth > 24 {
        return;
    }
    match t {
        Tag::Compound(entries) => {
            if let Some(id) = s(t, "id") {
                if let Some(n) = i(t, "amount") {
                    *fluids.entry(id.to_owned()).or_default() += n;
                    return;
                }
                if let Some(n) = i(t, "count").or_else(|| i(t, "Count")) {
                    *items.entry(id.to_owned()).or_default() += n;
                    return;
                }
            }
            for (_, v) in entries {
                stacks_at(v, items, fluids, depth + 1);
            }
        }
        Tag::List(v) => {
            for x in v {
                stacks_at(x, items, fluids, depth + 1);
            }
        }
        _ => {}
    }
}

/// A text component as plain text: a JSON string (`"\"Name\""`), an object's `text` and `extra`, a
/// `translate` key's last part; anything else as it is.
pub fn plain_text(raw: &str) -> String {
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) => out.push_str(s),
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(o) => {
                if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                    out.push_str(t);
                } else if let Some(k) = o.get("translate").and_then(|t| t.as_str()) {
                    out.push_str(&humanize(k.rsplit('.').next().unwrap_or(k)));
                }
                if let Some(e) = o.get("extra") {
                    walk(e, out);
                }
            }
            _ => {}
        }
    }
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => {
            let mut out = String::new();
            walk(&v, &mut out);
            out
        }
        Err(_) => raw.to_owned(),
    }
}

/// `create:mechanical_press` as "Mechanical press".
pub fn humanize(id: &str) -> String {
    let path = id.rsplit(':').next().unwrap_or(id);
    let mut out = String::with_capacity(path.len());
    for (n, w) in path.split(['_', '/']).filter(|w| !w.is_empty()).enumerate() {
        if n > 0 {
            out.push(' ');
        }
        if n == 0 {
            let mut c = w.chars();
            if let Some(first) = c.next() {
                out.extend(first.to_uppercase());
                out.push_str(c.as_str());
            }
        } else {
            out.push_str(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_components_read_as_plain_text() {
        assert_eq!(plain_text("\"Unnamed Train\""), "Unnamed Train");
        assert_eq!(plain_text(r#"{"text":"402 ","extra":[{"text":"Blue"}]}"#), "402 Blue");
        assert_eq!(plain_text("not json"), "not json");
    }

    #[test]
    fn ids_read_as_names() {
        assert_eq!(humanize("create:mechanical_press"), "Mechanical press");
        assert_eq!(humanize("minecraft:iron_ingot"), "Iron ingot");
    }

    #[test]
    fn uuids_print_as_minecraft_does() {
        assert_eq!(uuid_text(&[0x12345678, -1, 0, 1]), "12345678-ffff-ffff-0000-000000000001");
    }

    #[test]
    fn stacks_sum_items_and_fluids() {
        let st = |id: &str, n: i32| {
            Tag::Compound(vec![("id".into(), Tag::String(id.into())), ("count".into(), Tag::Int(n))])
        };
        let fl =
            Tag::Compound(vec![("id".into(), Tag::String("minecraft:water".into())), ("amount".into(), Tag::Int(500))]);
        let t = Tag::Compound(vec![
            (
                "items".into(),
                Tag::Compound(vec![("0".into(), st("minecraft:coal", 64)), ("1".into(), st("minecraft:coal", 3))]),
            ),
            ("tank".into(), Tag::List(vec![fl])),
        ]);
        let (mut a, mut b) = (BTreeMap::new(), BTreeMap::new());
        stacks(&t, &mut a, &mut b);
        assert_eq!(a["minecraft:coal"], 67);
        assert_eq!(b["minecraft:water"], 500);
    }
}
