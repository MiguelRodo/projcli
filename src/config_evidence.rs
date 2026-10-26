//! Bounded, redacted fingerprints of merged analysis configuration; no retained config copy.
use std::io::{self,Write};
use serde::{Deserialize,Serialize};
use serde_json::Value;
const MAX_BYTES:usize=4*1024*1024;
const MAX_NODES:usize=100_000;
const MAX_DEPTH:usize=64;

/// Opaque startup evidence, with explicit omissions and no partial digest on limits.
#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fingerprint {
    scope:String,
    status:String,
    blake3:Option<String>,
    redacted_values:usize,
}

/// Stream only bounded typed canonical tokens into BLAKE3, never into a retained config buffer.
struct Canonical {
    hash:blake3::Hasher,
    bytes:usize,
    nodes:usize,
    redactions:usize,
}
fn limit()->io::Error {io::Error::other("Configuration fingerprint limit")}
impl Write for Canonical {
    fn write(&mut self,bytes:&[u8])->io::Result<usize> {
        self.bytes=self.bytes.checked_add(bytes.len()).ok_or_else(limit)?;
        if self.bytes>MAX_BYTES{return Err(limit())}
        self.hash.update(bytes);Ok(bytes.len())
    }
    fn flush(&mut self)->io::Result<()> {Ok(())}
}
fn sensitive(key:&str)->bool {
    let key:String=key.chars().filter(|c|c.is_ascii_alphanumeric()).map(|c|c.to_ascii_lowercase()).collect();
    ["token","password","passwd","secret","credential","credentials","signature"].iter().any(|suffix|key.ends_with(suffix))
        ||matches!(key.as_str(),"auth"|"pass"|"bearer"|"pwd"|"authorization"|"key"|"apikey"|"privatekey"|"accesskey"|"accesskeyid"|"awsaccesskeyid"|"sig")
}
fn query_key(key:&str)->Option<String> {
    if key.len()>256{return None}
    let mut decoded=Vec::new();let mut bytes=key.bytes();
    while let Some(byte)=bytes.next() {
        decoded.push(if byte==b'%' {
            let a=(bytes.next()? as char).to_digit(16)?;
            let b=(bytes.next()? as char).to_digit(16)?;(a*16+b) as u8
        }else{byte});
    }
    String::from_utf8(decoded).ok()
}
fn private_url(text:&str)->bool {
    let rest=if let Some(rest)=text.strip_prefix("//"){rest}else if let Some((scheme,rest))=text.split_once("://") {
        if scheme.is_empty()||!scheme.starts_with(|c:char|c.is_ascii_alphabetic())
            ||!scheme.chars().all(|c|c.is_ascii_alphanumeric()||matches!(c,'+'|'-'|'.')) {return true}
        rest
    }else{
        if let Some((scheme,tail))=text.split_once(':') {
            // Native absolute Windows paths are configuration paths, not uncertain URI forms.
            if scheme.len()==1&&scheme.as_bytes()[0].is_ascii_alphabetic()&&tail.starts_with(['/', '\\']){return false}
            return !scheme.is_empty()&&scheme.starts_with(|c:char|c.is_ascii_alphabetic())
                &&scheme.chars().all(|c|c.is_ascii_alphanumeric()||matches!(c,'+'|'-'|'.'));
        }
        return false
    };
    if text.chars().any(|c|c.is_control()||c.is_whitespace())||text.contains('\\'){return true}
    let mut bytes=text.bytes();
    while let Some(byte)=bytes.next() {
        if byte==b'%'&&(!bytes.next().is_some_and(|b|b.is_ascii_hexdigit())
            ||!bytes.next().is_some_and(|b|b.is_ascii_hexdigit())) {return true}
    }
    let authority=rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()||authority.contains('@'){return true}
    if authority.starts_with('[') {
        let Some((_,tail))=authority.split_once(']') else{return true};
        if !tail.is_empty()&&!tail.strip_prefix(':').is_some_and(|port|!port.is_empty()&&port.bytes().all(|b|b.is_ascii_digit())){return true}
    }else if let Some((_,port))=authority.split_once(':') {
        if port.is_empty()||!port.bytes().all(|b|b.is_ascii_digit()){return true}
    }
    rest.split(['?', '#']).skip(1).any(|part|part.split('&').any(|pair|{
        if !pair.is_empty()&&!pair.contains('='){return true}
        let key=pair.split('=').next().unwrap_or("");
        query_key(key).is_none_or(|key|sensitive(&key))
    }))
}
impl Canonical {
    fn string(&mut self,text:&str)->io::Result<()> {
        if text.len()>MAX_BYTES{return Err(limit())}
        serde_json::to_writer(self,text).map_err(|_|limit())
    }
    fn node(&mut self,depth:usize)->io::Result<()> {
        self.nodes=self.nodes.checked_add(1).ok_or_else(limit)?;
        if depth>MAX_DEPTH||self.nodes>MAX_NODES{return Err(limit())}Ok(())
    }
    fn value(&mut self,value:&Value,depth:usize,secret_keys:bool)->io::Result<()> {
        self.node(depth)?;
        match value {
            Value::Object(map)=>{
                if map.len()>MAX_NODES-self.nodes{return Err(limit())}
                self.write_all(b"{")?;
                let mut keys:Vec<_>=map.keys().collect();keys.sort_unstable();
                for key in keys {
                    self.string(key)?;self.write_all(b":")?;
                    if secret_keys&&sensitive(key) {self.node(depth+1)?;self.redactions+=1;self.write_all(b"r")?}
                    else{self.value(&map[key],depth+1,secret_keys)?}
                    self.write_all(b";")?;
                }
                self.write_all(b"}")
            },
            Value::Array(values)=>{
                if values.len()>MAX_NODES-self.nodes{return Err(limit())}
                self.write_all(b"[")?;
                for value in values {self.value(value,depth+1,secret_keys)?;}
                self.write_all(b"]")
            },
            Value::String(text)=>{
                if text.len()>MAX_BYTES{return Err(limit())}
                if private_url(text) {self.redactions+=1;self.write_all(b"r")}
                else{self.write_all(b"s")?;self.string(text)}
            },
            Value::Number(number)=>{self.write_all(b"d")?;self.write_all(number.to_string().as_bytes())?;self.write_all(b";")},
            Value::Bool(value)=>self.write_all(if *value {b"t"}else{b"f"}),
            Value::Null=>self.write_all(b"n"),
        }
    }
}
impl Fingerprint {
    /// Fingerprint alias-normalized merged parameters, directories, ordered scripts/hooks and
    /// directory declaration order. Omit provider/auth/metadata/dev-only config. Redact sensitive
    /// parameter subtrees and credential-bearing/uncertain URLs before hashing. No raw value,
    /// key, path or URL is retained. This recognizer is not universal secret detection.
    pub(crate) fn capture(combined:&Value)->Self {
        let mut writer=Canonical {hash:blake3::Hasher::new_derive_key("projcli merged analysis configuration v1"),
            bytes:0,nodes:0,redactions:0};
        let empty=serde_json::json!({});let null=Value::Null;let isolated=Value::String("isolated".into());
        let result=(||->io::Result<()>{
            for path in ["/directories","/parameters","/build/scripts","/build/hooks/pre","/build/hooks/both","/build/hooks/post","/config/r/session","/config/r/rscript"] {
                writer.string(path)?;
                let default=if matches!(path,"/directories"|"/parameters"){&empty}else if path=="/config/r/session"{&isolated}else{&null};
                writer.value(combined.pointer(path).unwrap_or(default),0,path=="/parameters")?;
            }
            writer.write_all(b"directory_order[")?;
            if let Some(map)=combined.get("directories").and_then(Value::as_object) {
                for key in map.keys() {writer.node(1)?;writer.string(key)?;writer.write_all(b";")?;}
            }
            writer.write_all(b"]")
        })();
        Self {scope:"merged_analysis_and_effective_policy_v1".into(),
            status:if result.is_err(){"limit"}else if writer.redactions>0{"observed_redacted"}else{"observed"}.into(),
            blake3:result.ok().map(|_|writer.hash.finalize().to_hex().to_string()),redacted_values:writer.redactions}
    }

    /// Bind effective CLI/merged policies without re-reading mutable YAML or retaining parameters.
    pub(crate) fn bind_policy(mut self,policy:&str)->Self {
        if let Some(analysis)=self.blake3.take() {
            let mut hash=blake3::Hasher::new_derive_key("projcli effective analysis configuration v1");
            hash.update(analysis.as_bytes());hash.update(policy.as_bytes());
            self.blake3=Some(hash.finalize().to_hex().to_string());
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hash(value:&Value)->Option<String> {Fingerprint::capture(value).bind_policy("policy").blake3}
    #[test]
    fn canonical_order_types_sequences_and_redactions_are_unambiguous() {
        let a:Value=serde_json::from_str(r#"{"parameters":{"b":2,"a":1}}"#).unwrap();
        let b:Value=serde_json::from_str(r#"{"parameters":{"a":1,"b":2}}"#).unwrap();
        assert_eq!(hash(&a),hash(&b));
        assert_ne!(hash(&a),hash(&serde_json::json!({"parameters":{"b":2,"a":"1"}})));
        assert_ne!(hash(&serde_json::json!({"build":{"scripts":["a.py","b.py"]}})),
            hash(&serde_json::json!({"build":{"scripts":["b.py","a.py"]}})));
        let dirs_a:Value=serde_json::from_str(r#"{"directories":{"cache-a":{"path":"a"},"cache-b":{"path":"b"}}}"#).unwrap();
        let dirs_b:Value=serde_json::from_str(r#"{"directories":{"cache-b":{"path":"b"},"cache-a":{"path":"a"}}}"#).unwrap();
        assert_ne!(hash(&dirs_a),hash(&dirs_b));
        for url in ["https://user:secret@private.invalid/a","https://host/a?%74oken=secret",
            "https://host/a?X-Amz-Credential=secret","https://host/a#access_token=secret","https://host/a?sig=secret","//user:password@host/path","https:credential","https://host/a%zz","https://user:password/a"] {
            assert!(private_url(url));
        }
        assert_ne!(hash(&serde_json::json!({"parameters":{"url":"https://host/data?id=1"}})),
            hash(&serde_json::json!({"parameters":{"url":"https://host/data?id=2"}})));
        let secret=serde_json::json!({"parameters":{"token":{"anything":"SECRET"},"url":"https://u:p@host/a"}});
        let changed=serde_json::json!({"parameters":{"token":"DIFFERENT","url":"https://a:b@other/b"}});
        assert_eq!(hash(&secret),hash(&changed));
        assert_eq!(Fingerprint::capture(&secret).redacted_values,2);
        for (a,b) in [("//u:pass@host/a","//v:other@host/a"),("https:password","https:other"),
            ("https://host/a%zz?token=x","https://host/a%qq?token=y")] {
            assert_eq!(hash(&serde_json::json!({"parameters":{"url":a}})),hash(&serde_json::json!({"parameters":{"url":b}})));
        }
        assert_ne!(hash(&secret),hash(&serde_json::json!({"parameters":{"token":"SECRET","url":"[redacted]"}})));
        assert!(!serde_json::to_string(&Fingerprint::capture(&secret)).unwrap().contains("SECRET"));
        assert_ne!(Fingerprint::capture(&a).bind_policy("one").blake3,Fingerprint::capture(&a).bind_policy("two").blake3);
    }
    #[test]
    fn r_session_and_executable_selection_are_bound_without_retaining_paths() {
        let default=serde_json::json!({});
        let isolated=serde_json::json!({"config":{"r":{"session":"isolated"}}});
        let shared=serde_json::json!({"config":{"r":{"session":"shared"}}});
        assert_eq!(hash(&default),hash(&isolated));
        assert_ne!(hash(&isolated),hash(&shared));
        let selected=serde_json::json!({"config":{"r":{"rscript":"/private-machine/Rscript"}}});
        assert_ne!(hash(&default),hash(&selected));
        let text=serde_json::to_string(&Fingerprint::capture(&selected)).unwrap();
        assert!(!text.contains("private-machine"));
    }
    #[test]
    fn limits_never_publish_a_prefix_digest() {
        let mut deep=Value::Null;for _ in 0..MAX_DEPTH+2 {deep=Value::Array(vec![deep]);}
        let redacted=Value::Array(vec![serde_json::json!({"auth":1,"pass":2,"bearer":3});MAX_NODES/4]);
        for value in [deep,redacted,Value::Array(vec![Value::Null;MAX_NODES+1]),Value::String("x".repeat(MAX_BYTES+1))] {
            let evidence=Fingerprint::capture(&serde_json::json!({"parameters":value})).bind_policy("policy");
            assert_eq!(evidence.status,"limit");assert!(evidence.blake3.is_none());
        }
    }
}
