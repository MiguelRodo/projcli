//! Replaceable projr CSV encoding; build capture does not depend on CSV syntax.
use anyhow::Context;
use crate::manifest::ManifestRow;

/// Decodes the exact four-column projr schema. Missing/corrupt fields are errors;
/// this function performs no filesystem access and never repairs history.
pub fn decode(bytes: &[u8]) -> anyhow::Result<Vec<ManifestRow>> {
    let mut reader = csv::Reader::from_reader(bytes);
    anyhow::ensure!(reader.headers()?.iter().eq(["label", "fn", "version", "hash"]),
        "manifest.csv requires columns label,fn,version,hash in that order");
    let mut rows = Vec::new();
    for row in reader.deserialize::<ManifestRow>() {
        let row = row.context("Invalid manifest.csv record")?;
        row.validate()?;
        rows.push(row);
    }
    Ok(rows)
}

/// Encodes quoted UTF-8 string fields with LF records, matching R write.csv's
/// four-column layout, including an explicit header for an empty table.
pub fn encode(rows: &[ManifestRow]) -> anyhow::Result<Vec<u8>> {
    let mut writer = csv::WriterBuilder::new().has_headers(false)
        .quote_style(csv::QuoteStyle::Always).from_writer(Vec::new());
    writer.write_record(["label", "fn", "version", "hash"])?;
    for row in rows {
        row.validate()?;
        writer.serialize(row)?;
    }
    Ok(writer.into_inner()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_r_csv_roundtrips_and_rejects_invalid_history() {
        let bytes = b"\"label\",\"fn\",\"version\",\"hash\"\n\"raw\",\"a,\"\"b\nc.csv\",\"v1.2.3\",\"900150983cd24fb0d6963f7d28e17f72\"\n\"docs\",\"\",\"v1.2.3\",\"\"\n";
        let rows = decode(bytes).unwrap();
        assert_eq!(encode(&rows).unwrap(), bytes);
        assert!(decode(b"label,fn,version,hash\nraw,a,v1.0.0\n").is_err());
        assert!(decode(b"label,fn,version,hash\nraw,../a,v1.0.0,900150983cd24fb0d6963f7d28e17f72\n").is_err());
        assert!(decode(b"label,fn,version,hash\nraw,a,v1.0.0,wrong\n").is_err());
        assert!(decode(b"fn,label,version,hash\n").is_err());
        assert!(decode(b"").is_err());
        assert_eq!(encode(&[]).unwrap(), b"\"label\",\"fn\",\"version\",\"hash\"\n");
    }
    #[test]
    fn unicode_labels_and_filenames_roundtrip_without_normalization() {
        let rows = vec![ManifestRow {
            label: "raw-\u{00e9}".into(),
            r#fn: "donn\u{00e9}es/\u{6570}\u{636e},\".csv".into(),
            version: "v1.2.3".into(),
            hash: "900150983cd24fb0d6963f7d28e17f72".into(),
        }];
        let bytes = encode(&rows).unwrap();
        assert_eq!(decode(&bytes).unwrap(), rows);
        assert!(std::str::from_utf8(&bytes).unwrap().contains("donn\u{00e9}es"));
    }

}
