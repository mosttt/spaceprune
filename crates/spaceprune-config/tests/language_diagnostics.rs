//! Check detailed diagnostics at their production origins without changing user data.

use std::fs;

use spaceprune_config::{ConfigLoader, ConfigStore};
use spaceprune_contracts::{
    bundle::validate_bundle_paths, config::DailyTime, messages::set_display_language,
};

#[test]
fn detailed_configuration_errors_follow_language_and_preserve_literal_values() {
    let temporary = tempfile::tempdir().unwrap();
    let recipe = temporary.path().join("recipe-{source}.toml");
    let application = temporary.path().join("app-{path}.toml");
    fs::write(
        &application,
        "schema-version = 2\nrecipe = 'fixture'\n[theme]\nid = 42\n",
    )
    .unwrap();
    let original_application = fs::read(&application).unwrap();
    let store = ConfigStore::new(&application);
    fs::write(
        temporary.path().join("base.toml"),
        "schema-version = 2\nname = '用户名称 {name}'\n[modules.cache]\nname = 'Cache'\norder = 1\n",
    )
    .unwrap();
    for (language, reserved, expression, conflict, theme, bundle, time) in [
        (
            "en",
            "reserved for a management command",
            "Invalid variable expression `${BAD-NAME}`",
            "base is integer, overlay is string",
            "[theme].id must be a string",
            "conflicts with",
            "daily time must use zero-padded HH:MM",
        ),
        (
            "zh-CN",
            "已保留给管理命令",
            "变量表达式 `${BAD-NAME}` 无效",
            "基础值为整数，覆盖值为字符串",
            "[theme].id 必须是字符串",
            "冲突",
            "每日时间必须使用补零的 HH:MM 格式",
        ),
    ] {
        set_display_language(language);
        for (source, expected) in [
            (
                "schema-version = 2\nname = '用户名称 {name}'\n[modules.scan]\nname = 'Scan'\n",
                reserved,
            ),
            (
                "schema-version = 2\nname = '用户名称 {name}'\n[modules.cache]\nname = 'Cache'\n[modules.cache.tasks.files]\nname = 'Files'\nhandler = 'builtin.filesystem/paths'\n[modules.cache.tasks.files.options]\nroots = [{path = '${BAD-NAME}/cache'}]\n",
                expression,
            ),
            (
                "schema-version = 2\nname = '用户名称 {name}'\ninclude = ['base.toml']\n[modules.cache]\nname = 'Cache'\norder = 'first'\n",
                conflict,
            ),
        ] {
            fs::write(&recipe, source).unwrap();
            let error = ConfigLoader::new()
                .load_path(&recipe)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("recipe-{source}.toml"), "{error}");
            assert_eq!(fs::read_to_string(&recipe).unwrap(), source);
        }
        let error = store.load_theme_id().unwrap_err().to_string();
        assert!(error.contains(theme), "{error}");
        assert!(error.contains("app-{path}.toml"));
        let error = validate_bundle_paths(["parts-{path}/a.toml", "Parts-{path}/b.toml"])
            .unwrap_err()
            .to_string();
        assert!(error.contains(bundle), "{error}");
        assert!(error.contains("parts-{path}"));
        assert!(error.contains("Parts-{path}"));
        let error = toml::Value::String("3:7".into())
            .try_into::<DailyTime>()
            .unwrap_err()
            .to_string();
        assert!(error.contains(time), "{error}");
        fs::write(&recipe, "schema-version = 2\nname = '用户名称 {name}'\n").unwrap();
        assert_eq!(
            ConfigLoader::new().load_path(&recipe).unwrap().name,
            "用户名称 {name}"
        );
    }
    set_display_language("en");
    assert_eq!(fs::read(&application).unwrap(), original_application);
}
