//! DebugState `settings` (S2 §8): the pages as data plus the rects of the window's last frame.

use std::collections::HashMap;
use std::path::Path;

use super::form::{self, CodexModels, FieldId, Notice, TestState};
use super::Page;
use crate::debug_state::rects::{in_frame, Rect4, RectId};
use crate::debug_state::{
    AppearanceState, SettingsField, SettingsLanguage, SettingsOption, SettingsOther, SettingsPage,
    SettingsState, SettingsTest,
};
use crate::i18n::Language;
use crate::settings::Settings;

/// What the window holds, without gpui handles.
pub struct PageState {
    /// The page shown.
    pub page: Page,
    /// The 「外观」 page (`appearance::state`).
    pub appearance: AppearanceState,
    pub open_menu: Option<FieldId>,
    /// (field, text, trying).
    pub other: Option<(FieldId, String, bool)>,
    pub notice: Option<Notice>,
    pub codex: CodexModels,
    pub test: TestState,
}

pub struct FileView<'a> {
    pub error: Option<&'a str>,
    pub write_error: Option<&'a str>,
    pub path: &'a Path,
}

pub fn build(
    page: &PageState,
    settings: &Settings,
    file: FileView,
    id: Option<u64>,
    key: bool,
    titlebar: f32,
    rects: &HashMap<RectId, Rect4>,
) -> SettingsState {
    let at = |r: RectId| rects.get(&r).map(|x| in_frame(*x, titlebar));
    let fields = form::fields(&settings.monitor, &page.codex)
        .into_iter()
        .map(|f| {
            let i = f.id.index();
            SettingsField {
                id: f.id.name(),
                label: f.label,
                value: f.value,
                hint: f.hint,
                open: page.open_menu == Some(f.id),
                options: f.options.into_iter().enumerate().map(|(j, o)| SettingsOption { label: o.label, selected: o.selected, rect: at(RectId::SettingsOption(i, j)) }).collect(),
                rect: at(RectId::SettingsField(i)),
            }
        })
        .collect();
    let test = match &page.test {
        TestState::Idle => SettingsTest { state: "idle", text: String::new() },
        TestState::Running => SettingsTest { state: "running", text: String::new() },
        TestState::Done { ok, text } => SettingsTest { state: if *ok { "ok" } else { "failed" }, text: text.clone() },
    };
    SettingsState {
        id,
        key,
        page: page.page.id(),
        language: settings.language.id(),
        language_source: if settings.language_setting.is_some() { "config" } else { "system" },
        languages: Language::ALL
            .iter()
            .enumerate()
            .map(|(i, language)| SettingsLanguage {
                id: language.id(),
                label: language.label(),
                selected: *language == settings.language,
                rect: at(RectId::SettingsLanguage(i)),
            })
            .collect(),
        readonly: file.error.is_some(),
        error: file.error.map(str::to_string),
        write_error: file.write_error.map(str::to_string),
        config_path: file.path.display().to_string(),
        fields,
        other: page.other.as_ref().map(|(f, text, trying)| SettingsOther { field: f.name(), text: text.clone(), trying: *trying, rect: at(RectId::SettingsOther) }),
        notice: page.notice.as_ref().map(|n| n.text.clone()),
        notice_error: page.notice.as_ref().is_some_and(|n| n.error),
        test,
        pages: Page::ALL
            .iter()
            .map(|&p| SettingsPage {
                id: p.id(),
                label: p.label(settings.language),
                selected: p == page.page,
                rect: at(RectId::SettingsNav(p.index())),
            })
            .collect(),
        appearance: page.appearance.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MonitorSettings;

    fn settings(monitor: MonitorSettings) -> Settings {
        Settings {
            monitor,
            ..Settings::default()
        }
    }

    fn appearance() -> AppearanceState {
        use super::super::appearance_model::PickerModel;
        let m = PickerModel::open(Vec::new(), &gilvt_theme::Selection::system(), false);
        super::super::appearance::state(&m, 0, &|_| None)
    }

    fn page() -> PageState {
        PageState { page: Page::Monitor, appearance: appearance(), open_menu: None, other: None, notice: None, codex: CodexModels::Loading, test: TestState::Idle }
    }

    fn file(error: Option<&str>) -> FileView<'_> {
        FileView { error, write_error: None, path: Path::new("/h/.config/gilvt/config.toml") }
    }

    #[test]
    fn fields_values_and_rects() {
        let mut m = MonitorSettings::default();
        m.model = "sonnet".into();
        let mut rects = HashMap::new();
        rects.insert(
            RectId::SettingsField(FieldId::Model.index()),
            [200.0, 100.0, 180.0, 22.0],
        );
        rects.insert(
            RectId::SettingsOption(FieldId::Provider.index(), 1),
            [260.0, 70.0, 50.0, 20.0],
        );
        let s = build(
            &page(),
            &settings(m),
            file(None),
            Some(42),
            true,
            28.0,
            &rects,
        );
        assert_eq!(
            (s.id, s.key, s.page, s.readonly),
            (Some(42), true, "monitor", false)
        );
        assert_eq!(s.config_path, "/h/.config/gilvt/config.toml");
        let model = s.fields.iter().find(|f| f.id == "model").unwrap();
        assert_eq!(model.value, serde_json::json!("sonnet"));
        assert_eq!(model.label, "sonnet");
        assert_eq!(model.rect, Some([200.0, 128.0, 180.0, 22.0]), "moved below the titlebar");
        assert!(!model.open && model.options.iter().all(|o| o.rect.is_none()), "a closed dropdown's items are not drawn");
        let provider = s.fields.iter().find(|f| f.id == "provider").unwrap();
        assert_eq!(provider.options[1].label, "Codex");
        assert_eq!(provider.options[1].rect, Some([260.0, 98.0, 50.0, 20.0]));
        assert_eq!(s.fields.iter().map(|f| f.id).collect::<Vec<_>>(), FieldId::ALL.iter().map(|f| f.name()).collect::<Vec<_>>());
        assert_eq!(s.test, SettingsTest { state: "idle", text: String::new() });
    }

    #[test]
    fn pages_in_the_nav() {
        let mut p = page();
        p.page = Page::Appearance;
        let rects = HashMap::from([(RectId::SettingsNav(1), [8.0, 40.0, 134.0, 24.0])]);
        let s = build(
            &p,
            &settings(MonitorSettings::default()),
            file(None),
            None,
            true,
            28.0,
            &rects,
        );
        assert_eq!(s.page, "appearance");
        assert_eq!(s.language, "zh-CN");
        assert_eq!(s.language_source, "system", "no language in config.toml");
        assert_eq!(
            s.languages
                .iter()
                .map(|l| (l.id, l.selected))
                .collect::<Vec<_>>(),
            [("zh-CN", true), ("en", false)]
        );
        let pages: Vec<_> = s
            .pages
            .iter()
            .map(|p| (p.id, p.label, p.selected, p.rect))
            .collect();
        assert_eq!(
            pages,
            [
                ("appearance", "◐ 外观", true, None),
                (
                    "language",
                    "文A 语言",
                    false,
                    Some([8.0, 68.0, 134.0, 24.0])
                ),
                ("monitor", "◎ 监控官", false, None),
            ]
        );
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["appearance"]["mode"], "system");
        assert_eq!(json["appearance"]["rows"], serde_json::json!([]));
        assert_eq!(json["write_error"], serde_json::Value::Null);
    }

    #[test]
    fn english_language_page_has_stable_ids_labels_and_rects() {
        let mut p = page();
        p.page = Page::Language;
        let mut settings = settings(MonitorSettings::default());
        settings.language_setting = Some(Language::English);
        settings.language = Language::English;
        let rects = HashMap::from([
            (RectId::SettingsLanguage(0), [170.0, 60.0, 500.0, 40.0]),
            (RectId::SettingsLanguage(1), [170.0, 108.0, 500.0, 40.0]),
        ]);

        let state = build(&p, &settings, file(None), None, true, 28.0, &rects);
        assert_eq!(state.page, "language");
        assert_eq!(state.language, "en");
        assert_eq!(state.language_source, "config");
        assert_eq!(
            state
                .pages
                .iter()
                .map(|page| (page.id, page.label))
                .collect::<Vec<_>>(),
            [
                ("appearance", "◐ Appearance"),
                ("language", "文A Language"),
                ("monitor", "◎ Monitor")
            ]
        );
        assert_eq!(
            state
                .languages
                .iter()
                .map(|language| (
                    language.id,
                    language.label,
                    language.selected,
                    language.rect
                ))
                .collect::<Vec<_>>(),
            [
                ("zh-CN", "简体中文", false, Some([170.0, 88.0, 500.0, 40.0])),
                ("en", "English", true, Some([170.0, 136.0, 500.0, 40.0])),
            ]
        );
    }

    #[test]
    fn readonly_other_notice_and_test() {
        let mut p = page();
        p.open_menu = Some(FieldId::SummaryModel);
        p.other = Some((FieldId::SummaryModel, "missing-model".into(), true));
        p.notice = Some(Notice { error: true, text: "模型不存在或无权使用（…）".into() });
        p.test = TestState::Done { ok: false, text: "✗ 未找到 claude，请在设置里指定 CLI 路径".into() };
        let rects = HashMap::from([(RectId::SettingsOther, [10.0, 10.0, 200.0, 18.0])]);
        let s = build(
            &p,
            &settings(MonitorSettings::default()),
            file(Some("/h/config.toml: expected `=`")),
            None,
            false,
            0.0,
            &rects,
        );
        assert!(s.readonly);
        assert_eq!(s.error.as_deref(), Some("/h/config.toml: expected `=`"));
        assert!(s.fields.iter().find(|f| f.id == "summary_model").unwrap().open);
        assert_eq!(s.other, Some(SettingsOther { field: "summary_model", text: "missing-model".into(), trying: true, rect: Some([10.0, 10.0, 200.0, 18.0]) }));
        assert_eq!((s.notice.as_deref(), s.notice_error), (Some("模型不存在或无权使用（…）"), true));
        assert_eq!(s.test.state, "failed");
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["fields"][0]["id"], "enabled");
    }

    #[test]
    fn closed_window_is_absent_from_the_json() {
        let state = crate::debug_state::top_level(1, false, None, 0);
        assert!(serde_json::to_value(&state).unwrap().get("settings").is_none());
    }
}
