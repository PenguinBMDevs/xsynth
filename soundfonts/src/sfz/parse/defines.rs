use std::{borrow::Cow, collections::HashMap};

pub(super) fn apply_defines<'a>(value: &'a str, defines: &HashMap<String, String>) -> Cow<'a, str> {
    let mut value = Cow::Borrowed(value.trim());

    for (key, replace) in defines.iter() {
        if value.contains(key) {
            value = Cow::Owned(value.replace(key, replace));
        }
    }

    value
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::apply_defines;

    #[test]
    fn apply_defines_replaces_multiple_variables() {
        let defines = HashMap::from([
            ("$BASE".to_owned(), "samples".to_owned()),
            ("$NAME".to_owned(), "kick.wav".to_owned()),
        ]);

        let resolved = apply_defines(" $BASE/$NAME ", &defines);

        assert_eq!(resolved.as_ref(), "samples/kick.wav");
    }
}
