use rustyscript::{
    Module, Runtime, RuntimeOptions,
    deno_core::{extension, op2},
    serde_json,
};
use std::fs;

#[op2]
#[string]
fn command(
    #[serde] command: Vec<String>,
    #[serde] kwargs: serde_json::Value,
) -> String {
    "TODO".into()
}

extension!(
    slumber_extension,
    ops = [command],
    // esm_entry_point = "ext:slumber",
    esm = [ dir "../..", "slumber.d.ts" ],
);

fn main() {
    let extension = slumber_extension::init();

    // let slumber_module = Module::load("../../slumber.d.ts").unwrap();
    let slumber_module = Module::new(
        "slumber.ts",
        fs::read_to_string("../../slumber.d.ts").unwrap(),
    );
    let module = Module::load("../../slumber.ts").unwrap();
    let mut runtime = Runtime::new(RuntimeOptions {
        extensions: vec![extension],
        ..Default::default()
    })
    .unwrap();

    runtime.load_module(&slumber_module).unwrap();
    let handle = runtime.load_module(&module).unwrap();
    runtime
        .register_function("foo", |args| {
            if let Some(value) = args.first() {
                println!("called with: {value}");
            }
            Ok(serde_json::Value::Null)
        })
        .unwrap();
    let recipes = runtime
        .get_value_immediate::<serde_json::Value>(Some(&handle), "recipes");
    println!("{recipes:?}");
}
