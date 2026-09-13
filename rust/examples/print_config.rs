//! Prints a loaded config as JSON: the Rust half of scripts/parity-config.sh.
//! Usage: cargo run --example print_config -- <config.toml>

use std::path::Path;

use serde_json::Value;
use taskrunner::config::load_config;
use taskrunner::js;

fn main() {
    let path = std::env::args().nth(1).expect("usage: print_config <config.toml>");
    match load_config(Path::new(&path)) {
        Ok(config) => {
            let json = serde_json::to_value(&config).expect("config serializes");
            println!("{}", serde_json::to_string_pretty(&js_numbers(json)).expect("json prints"));
        }
        Err(_) => println!("ERROR"),
    }
}

/// `JSON.stringify` prints 2.0 as 2; match it so the diff is about values.
fn js_numbers(value: Value) -> Value {
    match value {
        Value::Number(n) => serde_json::from_str(&js::number(&n)).unwrap_or(Value::Number(n)),
        Value::Array(items) => Value::Array(items.into_iter().map(js_numbers).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, js_numbers(v))).collect())
        }
        other => other,
    }
}
