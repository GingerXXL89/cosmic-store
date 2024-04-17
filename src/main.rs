// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use cosmic::{
    app::{message, Command, Core, Settings},
    cosmic_config::{self, CosmicConfigEntry},
    cosmic_theme, executor,
    iced::{
        event::{self, Event},
        futures::{self, SinkExt},
        keyboard::{Event as KeyEvent, Key, Modifiers},
        subscription::{self, Subscription},
        window, Alignment, Length,
    },
    theme, widget, Application, ApplicationExt, Element,
};
use rayon::prelude::*;
use std::{
    any::TypeId,
    cmp,
    collections::{BTreeMap, HashMap, VecDeque},
    env, process,
    sync::Arc,
    time::{self, Instant},
};

use app_info::{AppIcon, AppInfo};
mod app_info;

use appstream_cache::AppstreamCache;
mod appstream_cache;

use backend::{Backends, Package};
mod backend;

use config::{AppTheme, Config, CONFIG_VERSION};
mod config;

use icon_cache::{icon_cache_handle, icon_cache_icon};
mod icon_cache;

use key_bind::{key_binds, KeyBind};
mod key_bind;

mod localize;

use operation::{Operation, OperationKind};
mod operation;

mod stats;

const ICON_SIZE_SEARCH: u16 = 48;
const ICON_SIZE_PACKAGE: u16 = 64;
const ICON_SIZE_DETAILS: u16 = 128;
const SYSTEM_ID: &'static str = "__SYSTEM__";

const EDITORS_CHOICE: &'static [&'static str] = &[
    "com.slack.Slack",
    "org.telegram.desktop",
    "org.gnome.meld",
    "com.valvesoftware.Steam",
    "net.lutris.Lutris",
    "com.mattermost.Desktop",
    "com.visualstudio.code",
    "com.spotify.Client",
    "virt-manager",
    "org.signal.Signal",
    "org.chromium.Chromium",
];

/// Runs application with these settings
#[rustfmt::skip]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    localize::localize();

    let (config_handler, config) = match cosmic_config::Config::new(App::APP_ID, CONFIG_VERSION) {
        Ok(config_handler) => {
            let config = match Config::get_entry(&config_handler) {
                Ok(ok) => ok,
                Err((errs, config)) => {
                    log::info!("errors loading config: {:?}", errs);
                    config
                }
            };
            (Some(config_handler), config)
        }
        Err(err) => {
            log::error!("failed to create config handler: {}", err);
            (None, Config::default())
        }
    };

    let mut settings = Settings::default();
    settings = settings.theme(config.app_theme.theme());

    #[cfg(target_os = "redox")]
    {
        // Redox does not support resize if doing CSDs
        settings = settings.client_decorations(false);
    }

    //TODO: allow size limits on iced_winit
    //settings = settings.size_limits(Limits::NONE.min_width(400.0).min_height(200.0));

    let flags = Flags {
        config_handler,
        config,
    };
    cosmic::app::run::<App>(settings, flags)?;

    Ok(())
}

//TODO: make app ID a newtype
fn match_id(a: &str, b: &str) -> bool {
    a.trim_end_matches(".desktop") == b.trim_end_matches(".desktop")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    SearchActivate,
}

impl Action {
    pub fn message(&self) -> Message {
        match self {
            Self::SearchActivate => Message::SearchActivate,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Flags {
    config_handler: Option<cosmic_config::Config>,
    config: Config,
}

/// Messages that are used specifically by our [`App`].
#[derive(Clone, Debug)]
pub enum Message {
    AppTheme(AppTheme),
    Backends(Backends),
    CategoryResults(Category, Vec<SearchResult>),
    Config(Config),
    DialogCancel,
    ExplorePage(Option<ExplorePage>),
    ExploreResults(ExplorePage, Vec<SearchResult>),
    Installed(Vec<(&'static str, Package)>),
    Key(Modifiers, Key),
    OpenDesktopId(String),
    Operation(OperationKind, &'static str, String, Arc<AppInfo>),
    PendingComplete(u64),
    PendingError(u64, String),
    PendingProgress(u64, f32),
    SearchActivate,
    SearchClear,
    SearchInput(String),
    SearchResults(String, Vec<SearchResult>),
    SearchSubmit,
    SelectInstalled(usize),
    SelectUpdates(usize),
    SelectNone,
    SelectCategoryResult(usize),
    SelectExploreResult(ExplorePage, usize),
    SelectSearchResult(usize),
    SelectedScreenshot(usize, String, Vec<u8>),
    SelectedScreenshotShown(usize),
    SystemThemeModeChange(cosmic_theme::ThemeMode),
    ToggleContextPage(ContextPage),
    UpdateAll,
    Updates(Vec<(&'static str, Package)>),
    WindowClose,
    WindowNew,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextPage {
    Settings,
}

impl ContextPage {
    fn title(&self) -> String {
        match self {
            Self::Settings => fl!("settings"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialogPage {
    FailedOperation(u64),
}

// From https://specifications.freedesktop.org/menu-spec/latest/apa.html
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Category {
    AudioVideo,
    Development,
    Education,
    Game,
    Graphics,
    Network,
    Office,
    Science,
    Settings,
    System,
    Utility,
}

impl Category {
    fn id(&self) -> &'static str {
        match self {
            Self::AudioVideo => "AudioVideo",
            Self::Development => "Development",
            Self::Education => "Education",
            Self::Game => "Game",
            Self::Graphics => "Graphics",
            Self::Network => "Network",
            Self::Office => "Office",
            Self::Science => "Science",
            Self::Settings => "Settings",
            Self::System => "System",
            Self::Utility => "Utility",
        }
    }

    fn title(&self) -> String {
        //TODO: nice titles for categories
        self.id().to_string()
    }
}

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub enum NavPage {
    #[default]
    Explore,
    Create,
    Work,
    Develop,
    Learn,
    Game,
    Relax,
    Socialize,
    Utilities,
    Installed,
    Updates,
}

impl NavPage {
    fn all() -> &'static [Self] {
        &[
            Self::Explore,
            Self::Create,
            Self::Work,
            Self::Develop,
            Self::Learn,
            Self::Game,
            Self::Relax,
            Self::Socialize,
            Self::Utilities,
            Self::Installed,
            Self::Updates,
        ]
    }

    fn title(&self) -> String {
        match self {
            Self::Explore => fl!("explore"),
            Self::Create => fl!("create"),
            Self::Work => fl!("work"),
            Self::Develop => fl!("develop"),
            Self::Learn => fl!("learn"),
            Self::Game => fl!("game"),
            Self::Relax => fl!("relax"),
            Self::Socialize => fl!("socialize"),
            Self::Utilities => fl!("utilities"),
            Self::Installed => fl!("installed-apps"),
            Self::Updates => fl!("updates"),
        }
    }

    // From https://specifications.freedesktop.org/menu-spec/latest/apa.html
    fn category(&self) -> Option<Category> {
        match self {
            /*TODO: Categories:
            Science
            Settings
            System
            */
            Self::Create => Some(Category::Graphics),
            Self::Work => Some(Category::Office),
            Self::Develop => Some(Category::Development),
            Self::Learn => Some(Category::Education),
            Self::Game => Some(Category::Game),
            Self::Relax => Some(Category::AudioVideo),
            Self::Socialize => Some(Category::Network),
            Self::Utilities => Some(Category::Utility),
            _ => None,
        }
    }

    fn icon(&self) -> widget::icon::Icon {
        match self {
            Self::Explore => icon_cache_icon("store-home-symbolic", 16),
            Self::Create => icon_cache_icon("store-create-symbolic", 16),
            Self::Work => icon_cache_icon("store-work-symbolic", 16),
            Self::Develop => icon_cache_icon("store-develop-symbolic", 16),
            Self::Learn => icon_cache_icon("store-learn-symbolic", 16),
            Self::Game => icon_cache_icon("store-game-symbolic", 16),
            Self::Relax => icon_cache_icon("store-relax-symbolic", 16),
            Self::Socialize => icon_cache_icon("store-socialize-symbolic", 16),
            Self::Utilities => icon_cache_icon("store-utilities-symbolic", 16),
            Self::Installed => icon_cache_icon("store-installed-symbolic", 16),
            Self::Updates => icon_cache_icon("store-updates-symbolic", 16),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ExplorePage {
    EditorsChoice,
    PopularApps,
    NewApps,
    RecentlyUpdated,
}

impl ExplorePage {
    fn all() -> &'static [Self] {
        &[
            Self::EditorsChoice,
            Self::PopularApps,
            Self::NewApps,
            Self::RecentlyUpdated,
        ]
    }

    fn title(&self) -> String {
        match self {
            Self::EditorsChoice => fl!("editors-choice"),
            Self::PopularApps => fl!("popular-apps"),
            Self::NewApps => fl!("new-apps"),
            Self::RecentlyUpdated => fl!("recently-updated"),
        }
    }
}

impl Package {
    pub fn card_view<'a>(
        &'a self,
        controls: Vec<Element<'a, Message>>,
        spacing: &cosmic_theme::Spacing,
    ) -> Element<'a, Message> {
        let width = 360.0 + 2.0 * spacing.space_s as f32;
        let mut height = 88.0 + 2.0 * spacing.space_xxs as f32;
        let mut column = widget::column::with_children(vec![
            widget::text::body(&self.info.name)
                .height(Length::Fixed(20.0))
                .into(),
            widget::text::caption(&self.info.summary)
                .height(Length::Fixed(28.0))
                .into(),
            //TODO: combine origins
            widget::text::caption(&self.info.source_name).into(),
            widget::text::caption(&self.version).into(),
        ]);
        if !controls.is_empty() {
            column = column
                .push(widget::vertical_space(Length::Fixed(
                    spacing.space_xxs.into(),
                )))
                .push(
                    widget::row::with_children(controls)
                        .height(Length::Fixed(32.0))
                        .spacing(spacing.space_xs),
                );
            height += spacing.space_xxs as f32 + 32.0;
        }

        widget::container(
            widget::row::with_children(vec![
                widget::icon::icon(self.icon.clone())
                    .size(ICON_SIZE_PACKAGE)
                    .into(),
                column.into(),
            ])
            .align_items(Alignment::Center)
            .spacing(spacing.space_s),
        )
        .center_y()
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .padding([spacing.space_xxs, spacing.space_s])
        .style(theme::Container::Card)
        .into()
    }
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    backend_name: &'static str,
    id: String,
    icon: widget::icon::Handle,
    info: Arc<AppInfo>,
    weight: i64,
}

impl SearchResult {
    pub fn card_view<'a>(&'a self, spacing: &cosmic_theme::Spacing) -> Element<'a, Message> {
        widget::container(
            widget::row::with_children(vec![
                widget::icon::icon(self.icon.clone())
                    .size(ICON_SIZE_SEARCH)
                    .into(),
                widget::column::with_children(vec![
                    widget::text::body(&self.info.name)
                        .height(Length::Fixed(20.0))
                        .into(),
                    widget::text::caption(&self.info.summary)
                        .height(Length::Fixed(28.0))
                        .into(),
                    //TODO: Combine origins
                    widget::text::caption(&self.info.source_name).into(),
                ])
                .into(),
            ])
            .align_items(Alignment::Center)
            .spacing(spacing.space_s),
        )
        .center_y()
        .width(Length::Fixed(240.0 + (spacing.space_s as f32) * 2.0))
        .height(Length::Fixed(62.0 + (spacing.space_xxs as f32) * 2.0))
        .padding([spacing.space_xxs, spacing.space_s])
        .style(theme::Container::Card)
        .into()
    }
}

#[derive(Clone, Debug)]
pub struct Selected {
    backend_name: &'static str,
    id: String,
    icon: widget::icon::Handle,
    info: Arc<AppInfo>,
    screenshot_images: HashMap<usize, widget::image::Handle>,
    screenshot_shown: usize,
}

/// The [`App`] stores application-specific state.
pub struct App {
    core: Core,
    config_handler: Option<cosmic_config::Config>,
    config: Config,
    locale: String,
    app_themes: Vec<String>,
    backends: Backends,
    context_page: ContextPage,
    dialog_pages: VecDeque<DialogPage>,
    explore_page_opt: Option<ExplorePage>,
    key_binds: HashMap<KeyBind, Action>,
    nav_model: widget::nav_bar::Model,
    pending_operation_id: u64,
    pending_operations: BTreeMap<u64, (Operation, f32)>,
    failed_operations: BTreeMap<u64, (Operation, String)>,
    search_active: bool,
    search_id: widget::Id,
    search_input: String,
    installed: Option<Vec<(&'static str, Package)>>,
    updates: Option<Vec<(&'static str, Package)>>,
    waiting_installed: Vec<(&'static str, String, String)>,
    waiting_updates: Vec<(&'static str, String, String)>,
    category_results: Option<(Category, Vec<SearchResult>)>,
    explore_results: HashMap<ExplorePage, Vec<SearchResult>>,
    search_results: Option<(String, Vec<SearchResult>)>,
    selected_opt: Option<Selected>,
}

impl App {
    fn open_desktop_id(&self, mut desktop_id: String) -> Command<Message> {
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if !desktop_id.ends_with(".desktop") {
                        desktop_id.push_str(".desktop");
                    }
                    let xdg_dirs = match xdg::BaseDirectories::with_prefix("applications") {
                        Ok(ok) => ok,
                        Err(err) => {
                            log::warn!("failed to find applications xdg directories: {}", err);
                            return message::none();
                        }
                    };
                    let path = match xdg_dirs.find_data_file(&desktop_id) {
                        Some(some) => some,
                        None => {
                            log::warn!("failed to find desktop file for {:?}", desktop_id);
                            return message::none();
                        }
                    };
                    let entry = match freedesktop_entry_parser::parse_entry(&path) {
                        Ok(ok) => ok,
                        Err(err) => {
                            log::warn!("failed to read desktop file {:?}: {}", path, err);
                            return message::none();
                        }
                    };
                    //TODO: handlne Terminal=true
                    let exec = match entry.section("Desktop Entry").attr("Exec") {
                        Some(some) => some,
                        None => {
                            log::warn!("no exec section in {:?}", path);
                            return message::none();
                        }
                    };
                    //TODO: use libcosmic for loading desktop data
                    cosmic::desktop::spawn_desktop_exec(exec, Vec::<(&str, &str)>::new());
                    message::none()
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn operation(&mut self, operation: Operation) {
        let id = self.pending_operation_id;
        self.pending_operation_id += 1;
        self.pending_operations.insert(id, (operation, 0.0));
    }

    fn generic_search<F: Fn(&str, &AppInfo) -> Option<i64> + Send + Sync>(
        backends: &Backends,
        filter_map: F,
    ) -> Vec<SearchResult> {
        let mut results = Vec::<SearchResult>::new();
        //TODO: par_iter?
        for (backend_name, backend) in backends.iter() {
            //TODO: par_iter?
            for appstream_cache in backend.info_caches() {
                let mut backend_results = appstream_cache
                    .infos
                    .par_iter()
                    .filter_map(|(id, info)| {
                        let weight = filter_map(id, info)?;
                        Some(SearchResult {
                            backend_name,
                            id: id.clone(),
                            icon: appstream_cache.icon(info),
                            info: info.clone(),
                            weight,
                        })
                    })
                    .collect();
                results.append(&mut backend_results);
            }
        }
        results.sort_by(|a, b| match a.weight.cmp(&b.weight) {
            cmp::Ordering::Equal => {
                match lexical_sort::natural_lexical_cmp(&a.info.name, &b.info.name) {
                    cmp::Ordering::Equal => {
                        lexical_sort::natural_lexical_cmp(&a.backend_name, &b.backend_name)
                    }
                    ordering => ordering,
                }
            }
            ordering => ordering,
        });
        results
    }

    fn category(&self, category: Category) -> Command<Message> {
        let backends = self.backends.clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let start = Instant::now();
                    let results = Self::generic_search(&backends, |_id, info| {
                        //TODO: contains doesn't work due to type mismatch
                        if info.categories.iter().any(|x| x == category.id()) {
                            Some(-(info.monthly_downloads as i64))
                        } else {
                            None
                        }
                    });
                    let duration = start.elapsed();
                    log::info!(
                        "searched for category {:?} in {:?}, found {} results",
                        category,
                        duration,
                        results.len()
                    );
                    message::app(Message::CategoryResults(category, results))
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn explore_results(&self, explore_page: ExplorePage) -> Command<Message> {
        let backends = self.backends.clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let start = Instant::now();
                    let results = Self::generic_search(&backends, |id, info| {
                        //TODO: use explore_page
                        match explore_page {
                            ExplorePage::EditorsChoice => EDITORS_CHOICE
                                .iter()
                                .position(|choice_id| match_id(choice_id, &id))
                                .map(|x| x as i64),
                            ExplorePage::PopularApps => Some(-(info.monthly_downloads as i64)),
                            _ => None,
                        }
                    });
                    let duration = start.elapsed();
                    log::info!(
                        "searched for {:?} in {:?}, found {} results",
                        explore_page,
                        duration,
                        results.len()
                    );
                    message::app(Message::ExploreResults(explore_page, results))
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn search(&self) -> Command<Message> {
        let input = self.search_input.clone();
        let pattern = regex::escape(&input);
        let regex = match regex::RegexBuilder::new(&pattern)
            .case_insensitive(true)
            .build()
        {
            Ok(ok) => ok,
            Err(err) => {
                log::warn!("failed to parse regex {:?}: {}", pattern, err);
                return Command::none();
            }
        };
        let backends = self.backends.clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let start = Instant::now();
                    let results = Self::generic_search(&backends, |_id, info| {
                        //TODO: improve performance
                        let stats_weight = |weight: i64| {
                            //TODO: make sure no overflows
                            (weight << 56) - (info.monthly_downloads as i64)
                        };
                        //TODO: fuzzy match (nucleus-matcher?)
                        match regex.find(&info.name) {
                            Some(mat) => {
                                if mat.range().start == 0 {
                                    if mat.range().end == info.name.len() {
                                        // Name equals search phrase
                                        Some(stats_weight(0))
                                    } else {
                                        // Name starts with search phrase
                                        Some(stats_weight(1))
                                    }
                                } else {
                                    // Name contains search phrase
                                    Some(stats_weight(2))
                                }
                            }
                            None => match regex.find(&info.summary) {
                                Some(mat) => {
                                    if mat.range().start == 0 {
                                        if mat.range().end == info.summary.len() {
                                            // Summary equals search phrase
                                            Some(stats_weight(3))
                                        } else {
                                            // Summary starts with search phrase
                                            Some(stats_weight(4))
                                        }
                                    } else {
                                        // Summary contains search phrase
                                        Some(stats_weight(5))
                                    }
                                }
                                None => match regex.find(&info.description) {
                                    Some(mat) => {
                                        if mat.range().start == 0 {
                                            if mat.range().end == info.summary.len() {
                                                // Description equals search phrase
                                                Some(stats_weight(6))
                                            } else {
                                                // Description starts with search phrase
                                                Some(stats_weight(7))
                                            }
                                        } else {
                                            // Description contains search phrase
                                            Some(stats_weight(8))
                                        }
                                    }
                                    None => None,
                                },
                            },
                        }
                    });
                    let duration = start.elapsed();
                    log::info!(
                        "searched for {:?} in {:?}, found {} results",
                        input,
                        duration,
                        results.len()
                    );
                    message::app(Message::SearchResults(input, results))
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn update_backends(&self) -> Command<Message> {
        let locale = self.locale.clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let start = Instant::now();
                    let backends = backend::backends(&locale);
                    let duration = start.elapsed();
                    log::info!("loaded backends in {:?}", duration);
                    message::app(Message::Backends(backends))
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn update_config(&mut self) -> Command<Message> {
        cosmic::app::command::set_theme(self.config.app_theme.theme())
    }

    fn update_installed(&self) -> Command<Message> {
        let backends = self.backends.clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut installed = Vec::new();
                    //TODO: par_iter?
                    for (backend_name, backend) in backends.iter() {
                        let start = Instant::now();
                        match backend.installed() {
                            Ok(packages) => {
                                for package in packages {
                                    installed.push((*backend_name, package));
                                }
                            }
                            Err(err) => {
                                log::error!("failed to list installed: {}", err);
                            }
                        }
                        let duration = start.elapsed();
                        log::info!("loaded installed from {} in {:?}", backend_name, duration);
                    }
                    installed.sort_by(|a, b| {
                        if a.1.id == SYSTEM_ID {
                            cmp::Ordering::Less
                        } else if b.1.id == SYSTEM_ID {
                            cmp::Ordering::Greater
                        } else {
                            lexical_sort::natural_lexical_cmp(&a.1.info.name, &b.1.info.name)
                        }
                    });
                    message::app(Message::Installed(installed))
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn update_updates(&self) -> Command<Message> {
        let backends = self.backends.clone();
        Command::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut updates = Vec::new();
                    //TODO: par_iter?
                    for (backend_name, backend) in backends.iter() {
                        let start = Instant::now();
                        match backend.updates() {
                            Ok(packages) => {
                                for package in packages {
                                    updates.push((*backend_name, package));
                                }
                            }
                            Err(err) => {
                                log::error!("failed to list updates: {}", err);
                            }
                        }
                        let duration = start.elapsed();
                        log::info!("loaded updates from {} in {:?}", backend_name, duration);
                    }
                    updates.sort_by(|a, b| {
                        if a.1.id == SYSTEM_ID {
                            cmp::Ordering::Less
                        } else if b.1.id == SYSTEM_ID {
                            cmp::Ordering::Greater
                        } else {
                            lexical_sort::natural_lexical_cmp(&a.1.info.name, &b.1.info.name)
                        }
                    });
                    message::app(Message::Updates(updates))
                })
                .await
                .unwrap_or(message::none())
            },
            |x| x,
        )
    }

    fn update_title(&mut self) -> Command<Message> {
        self.set_window_title(fl!("cosmic-app-store"))
    }

    fn settings(&self) -> Element<Message> {
        let app_theme_selected = match self.config.app_theme {
            AppTheme::Dark => 1,
            AppTheme::Light => 2,
            AppTheme::System => 0,
        };
        widget::settings::view_column(vec![widget::settings::view_section(fl!("appearance"))
            .add(
                widget::settings::item::builder(fl!("theme")).control(widget::dropdown(
                    &self.app_themes,
                    Some(app_theme_selected),
                    move |index| {
                        Message::AppTheme(match index {
                            1 => AppTheme::Dark,
                            2 => AppTheme::Light,
                            _ => AppTheme::System,
                        })
                    },
                )),
            )
            .into()])
        .into()
    }
}

/// Implement [`Application`] to integrate with COSMIC.
impl Application for App {
    /// Default async executor to use with the app.
    type Executor = executor::Default;

    /// Argument received
    type Flags = Flags;

    /// Message type specific to our [`App`].
    type Message = Message;

    /// The unique application ID to supply to the window manager.
    const APP_ID: &'static str = "com.system76.CosmicStore";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    /// Creates the application, and optionally emits command on initialize.
    fn init(core: Core, flags: Self::Flags) -> (Self, Command<Self::Message>) {
        let locale = sys_locale::get_locale().unwrap_or_else(|| {
            log::warn!("failed to get system locale, falling back to en-US");
            String::from("en-US")
        });

        let app_themes = vec![fl!("match-desktop"), fl!("dark"), fl!("light")];

        let mut nav_model = widget::nav_bar::Model::default();
        for &nav_page in NavPage::all() {
            let id = nav_model
                .insert()
                .icon(nav_page.icon())
                .text(nav_page.title())
                .data::<NavPage>(nav_page)
                .id();
            if nav_page == NavPage::default() {
                //TODO: save last page?
                nav_model.activate(id);
            }
        }

        let mut app = App {
            core,
            config_handler: flags.config_handler,
            config: flags.config,
            locale,
            app_themes,
            backends: Backends::new(),
            context_page: ContextPage::Settings,
            dialog_pages: VecDeque::new(),
            explore_page_opt: None,
            key_binds: key_binds(),
            nav_model,
            pending_operation_id: 0,
            pending_operations: BTreeMap::new(),
            failed_operations: BTreeMap::new(),
            search_active: false,
            search_id: widget::Id::unique(),
            search_input: String::new(),
            installed: None,
            updates: None,
            waiting_installed: Vec::new(),
            waiting_updates: Vec::new(),
            category_results: None,
            explore_results: HashMap::new(),
            search_results: None,
            selected_opt: None,
        };

        let command = Command::batch([app.update_title(), app.update_backends()]);
        (app, command)
    }

    fn nav_model(&self) -> Option<&widget::nav_bar::Model> {
        Some(&self.nav_model)
    }

    fn on_escape(&mut self) -> Command<Message> {
        if self.core.window.show_context {
            // Close context drawer if open
            self.core.window.show_context = false;
        } else if self.search_active {
            // Close search if open
            self.search_active = false;
            self.search_results = None;
        }
        Command::none()
    }

    fn on_nav_select(&mut self, id: widget::nav_bar::Id) -> Command<Message> {
        self.category_results = None;
        self.explore_page_opt = None;
        self.search_active = false;
        self.search_results = None;
        self.selected_opt = None;
        self.nav_model.activate(id);
        //TODO: do not preserve scroll on page change
        if let Some(category) = self
            .nav_model
            .active_data::<NavPage>()
            .and_then(|nav_page| nav_page.category())
        {
            return self.category(category);
        }
        Command::none()
    }

    /// Handle application events here.
    fn update(&mut self, message: Self::Message) -> Command<Message> {
        // Helper for updating config values efficiently
        macro_rules! config_set {
            ($name: ident, $value: expr) => {
                match &self.config_handler {
                    Some(config_handler) => {
                        match paste::paste! { self.config.[<set_ $name>](config_handler, $value) } {
                            Ok(_) => {}
                            Err(err) => {
                                log::warn!(
                                    "failed to save config {:?}: {}",
                                    stringify!($name),
                                    err
                                );
                            }
                        }
                    }
                    None => {
                        self.config.$name = $value;
                        log::warn!(
                            "failed to save config {:?}: no config handler",
                            stringify!($name)
                        );
                    }
                }
            };
        }

        match message {
            Message::AppTheme(app_theme) => {
                config_set!(app_theme, app_theme);
                return self.update_config();
            }
            Message::Backends(backends) => {
                self.backends = backends;
                return Command::batch([
                    self.update_installed(),
                    self.update_updates(),
                    self.explore_results(ExplorePage::EditorsChoice),
                    self.explore_results(ExplorePage::PopularApps),
                    //TODO: add more explore pages
                ]);
            }
            Message::CategoryResults(category, results) => {
                self.category_results = Some((category, results));
            }
            Message::Config(config) => {
                if config != self.config {
                    log::info!("update config");
                    //TODO: update syntax theme by clearing tabs, only if needed
                    self.config = config;
                    return self.update_config();
                }
            }
            Message::DialogCancel => {
                self.dialog_pages.pop_front();
            }
            Message::ExplorePage(explore_page_opt) => {
                self.explore_page_opt = explore_page_opt;
            }
            Message::ExploreResults(explore_page, results) => {
                self.explore_results.insert(explore_page, results);
            }
            Message::Installed(installed) => {
                self.installed = Some(installed);
                self.waiting_installed.clear();
            }
            Message::Key(modifiers, key) => {
                for (key_bind, action) in self.key_binds.iter() {
                    if key_bind.matches(modifiers, &key) {
                        return self.update(action.message());
                    }
                }
            }
            Message::OpenDesktopId(desktop_id) => {
                return self.open_desktop_id(desktop_id);
            }
            Message::Operation(kind, backend_name, package_id, info) => {
                self.operation(Operation {
                    kind,
                    backend_name,
                    package_id,
                    info,
                });
            }
            Message::PendingComplete(id) => {
                if let Some((op, _)) = self.pending_operations.remove(&id) {
                    self.waiting_installed.push((
                        op.backend_name,
                        op.info.source_id.clone(),
                        op.package_id.clone(),
                    ));
                    self.waiting_updates.push((
                        op.backend_name,
                        op.info.source_id.clone(),
                        op.package_id.clone(),
                    ));
                    //TODO: self.complete_operations.insert(id, op);
                }
                return Command::batch([self.update_installed(), self.update_updates()]);
            }
            Message::PendingError(id, err) => {
                log::warn!("operation {id} failed: {err}");
                if let Some((op, _)) = self.pending_operations.remove(&id) {
                    self.failed_operations.insert(id, (op, err));
                    self.dialog_pages.push_back(DialogPage::FailedOperation(id));
                }
            }
            Message::PendingProgress(id, new_progress) => {
                if let Some((_, progress)) = self.pending_operations.get_mut(&id) {
                    *progress = new_progress;
                }
            }
            Message::SearchActivate => {
                self.selected_opt = None;
                self.search_active = true;
                return widget::text_input::focus(self.search_id.clone());
            }
            Message::SearchClear => {
                self.search_active = false;
                self.search_input.clear();
                self.search_results = None;
            }
            Message::SearchInput(input) => {
                if input != self.search_input {
                    self.search_input = input;
                    // This performs live search
                    if !self.search_input.is_empty() {
                        return self.search();
                    }
                }
            }
            Message::SearchResults(input, results) => {
                if input == self.search_input {
                    self.search_results = Some((input, results));
                } else {
                    log::warn!(
                        "received {} results for {:?} after search changed to {:?}",
                        results.len(),
                        input,
                        self.search_input
                    );
                }
            }
            Message::SearchSubmit => {
                if !self.search_input.is_empty() {
                    return self.search();
                }
            }
            Message::SelectInstalled(installed_i) => {
                if let Some(installed) = &self.installed {
                    match installed
                        .get(installed_i)
                        .map(|(backend_name, package)| (backend_name, package.clone()))
                    {
                        Some((backend_name, package)) => {
                            log::info!("selected {:?}", package.id);
                            self.selected_opt = Some(Selected {
                                backend_name,
                                id: package.id,
                                icon: package.icon,
                                info: package.info,
                                screenshot_images: HashMap::new(),
                                screenshot_shown: 0,
                            });
                        }
                        None => {
                            log::error!(
                                "failed to find installed package with index {}",
                                installed_i
                            );
                        }
                    }
                }
            }
            Message::SelectUpdates(updates_i) => {
                if let Some(updates) = &self.updates {
                    match updates
                        .get(updates_i)
                        .map(|(backend_name, package)| (backend_name, package.clone()))
                    {
                        Some((backend_name, package)) => {
                            log::info!("selected {:?}", package.id);
                            self.selected_opt = Some(Selected {
                                backend_name,
                                id: package.id,
                                icon: package.icon,
                                info: package.info,
                                screenshot_images: HashMap::new(),
                                screenshot_shown: 0,
                            });
                        }
                        None => {
                            log::error!("failed to find updates package with index {}", updates_i);
                        }
                    }
                }
            }
            Message::SelectNone => {
                self.selected_opt = None;
            }
            Message::SelectCategoryResult(result_i) => {
                if let Some((_category, results)) = &self.category_results {
                    match results.get(result_i) {
                        Some(result) => {
                            log::info!("selected {:?}", result.id);
                            self.selected_opt = Some(Selected {
                                backend_name: result.backend_name,
                                id: result.id.clone(),
                                icon: result.icon.clone(),
                                info: result.info.clone(),
                                screenshot_images: HashMap::new(),
                                screenshot_shown: 0,
                            })
                        }
                        None => {
                            log::error!("failed to find category result with index {}", result_i);
                        }
                    }
                }
            }
            Message::SelectExploreResult(explore_page, result_i) => {
                if let Some(results) = self.explore_results.get(&explore_page) {
                    match results.get(result_i) {
                        Some(result) => {
                            log::info!("selected {:?}", result.id);
                            self.selected_opt = Some(Selected {
                                backend_name: result.backend_name,
                                id: result.id.clone(),
                                icon: result.icon.clone(),
                                info: result.info.clone(),
                                screenshot_images: HashMap::new(),
                                screenshot_shown: 0,
                            })
                        }
                        None => {
                            log::error!(
                                "failed to find {:?} result with index {}",
                                explore_page,
                                result_i
                            );
                        }
                    }
                }
            }
            Message::SelectSearchResult(result_i) => {
                if let Some((_input, results)) = &self.search_results {
                    match results.get(result_i) {
                        Some(result) => {
                            log::info!("selected {:?}", result.id);
                            self.selected_opt = Some(Selected {
                                backend_name: result.backend_name,
                                id: result.id.clone(),
                                icon: result.icon.clone(),
                                info: result.info.clone(),
                                screenshot_images: HashMap::new(),
                                screenshot_shown: 0,
                            })
                        }
                        None => {
                            log::error!("failed to find search result with index {}", result_i);
                        }
                    }
                }
            }
            Message::SelectedScreenshot(i, url, data) => {
                if let Some(selected) = &mut self.selected_opt {
                    if let Some(screenshot) = selected.info.screenshots.get(i) {
                        if screenshot.url == url {
                            selected
                                .screenshot_images
                                .insert(i, widget::image::Handle::from_memory(data));
                        }
                    }
                }
            }
            Message::SelectedScreenshotShown(i) => {
                if let Some(selected) = &mut self.selected_opt {
                    selected.screenshot_shown = i;
                }
            }
            Message::SystemThemeModeChange(_theme_mode) => {
                return self.update_config();
            }
            Message::ToggleContextPage(context_page) => {
                //TODO: ensure context menus are closed
                if self.context_page == context_page {
                    self.core.window.show_context = !self.core.window.show_context;
                } else {
                    self.context_page = context_page;
                    self.core.window.show_context = true;
                }
                self.set_context_title(context_page.title());
            }
            Message::UpdateAll => {
                if let Some(updates) = &self.updates {
                    //TODO: this shows multiple pkexec dialogs
                    let mut ops = Vec::with_capacity(updates.len());
                    for (backend_name, package) in updates.iter() {
                        ops.push(Operation {
                            kind: OperationKind::Update,
                            backend_name,
                            package_id: package.id.clone(),
                            info: package.info.clone(),
                        });
                    }
                    for op in ops {
                        self.operation(op);
                    }
                }
            }
            Message::Updates(updates) => {
                self.updates = Some(updates);
                self.waiting_updates.clear();
            }
            Message::WindowClose => {
                return window::close(window::Id::MAIN);
            }
            Message::WindowNew => match env::current_exe() {
                Ok(exe) => match process::Command::new(&exe).spawn() {
                    Ok(_child) => {}
                    Err(err) => {
                        log::error!("failed to execute {:?}: {}", exe, err);
                    }
                },
                Err(err) => {
                    log::error!("failed to get current executable path: {}", err);
                }
            },
        }

        Command::none()
    }

    fn context_drawer(&self) -> Option<Element<Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match self.context_page {
            ContextPage::Settings => self.settings(),
        })
    }

    fn dialog(&self) -> Option<Element<Message>> {
        let dialog_page = match self.dialog_pages.front() {
            Some(some) => some,
            None => return None,
        };

        let dialog = match dialog_page {
            DialogPage::FailedOperation(id) => {
                //TODO: try next dialog page (making sure index is used by Dialog messages)?
                let (operation, err) = self.failed_operations.get(id)?;

                let (title, body) = operation.failed_dialog(&err);
                widget::dialog(title)
                    .body(body)
                    .icon(widget::icon::from_name("dialog-error").size(64))
                    //TODO: retry action
                    .primary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
            }
        };

        Some(dialog.into())
    }

    fn header_start(&self) -> Vec<Element<Message>> {
        vec![if self.search_active {
            widget::text_input::search_input("", &self.search_input)
                .width(Length::Fixed(240.0))
                .id(self.search_id.clone())
                .on_clear(Message::SearchClear)
                .on_input(Message::SearchInput)
                .on_submit(Message::SearchSubmit)
                .into()
        } else {
            widget::button::icon(widget::icon::from_name("system-search-symbolic"))
                .on_press(Message::SearchActivate)
                .into()
        }]
    }

    /// Creates a view after each update.
    fn view(&self) -> Element<Self::Message> {
        let spacing = theme::active().cosmic().spacing;
        let cosmic_theme::Spacing {
            space_m,
            space_s,
            space_xs,
            space_xxs,
            ..
        } = spacing;

        let content: Element<_> = match &self.selected_opt {
            Some(selected) => {
                //TODO: more efficient checks
                let mut waiting_refresh = false;
                for (backend_name, source_id, package_id) in self
                    .waiting_installed
                    .iter()
                    .chain(self.waiting_updates.iter())
                {
                    if backend_name == &selected.backend_name
                        && source_id == &selected.info.source_id
                        && match_id(package_id, &selected.id)
                    {
                        waiting_refresh = true;
                        break;
                    }
                }
                let mut is_installed = false;
                if let Some(installed) = &self.installed {
                    for (backend_name, package) in installed {
                        if backend_name == &selected.backend_name
                            && &package.info.source_id == &selected.info.source_id
                            && match_id(&package.id, &selected.id)
                        {
                            is_installed = true;
                            break;
                        }
                    }
                }
                let mut update_opt = None;
                if let Some(updates) = &self.updates {
                    for (backend_name, package) in updates {
                        if backend_name == &selected.backend_name
                            && &package.info.source_id == &selected.info.source_id
                            && match_id(&package.id, &selected.id)
                        {
                            update_opt = Some(Message::Operation(
                                OperationKind::Update,
                                backend_name,
                                package.id.clone(),
                                package.info.clone(),
                            ));
                            break;
                        }
                    }
                }
                let mut progress_opt = None;
                for (_id, (op, progress)) in self.pending_operations.iter() {
                    if op.backend_name == selected.backend_name
                        && &op.info.source_id == &selected.info.source_id
                        && match_id(&op.package_id, &selected.id)
                    {
                        progress_opt = Some(*progress);
                        break;
                    }
                }

                let mut column = widget::column::with_capacity(2)
                    .padding([0, space_s])
                    .spacing(space_m)
                    .width(Length::Fill);
                column = column.push(
                    widget::button::standard(fl!("back"))
                        .leading_icon(icon_cache_handle("go-previous-symbolic", 16))
                        .on_press(Message::SelectNone),
                );
                let mut buttons = Vec::with_capacity(2);
                if let Some(progress) = progress_opt {
                    //TODO: get height from theme?
                    buttons.push(
                        widget::progress_bar(0.0..=100.0, progress)
                            .height(Length::Fixed(4.0))
                            .into(),
                    )
                } else if waiting_refresh {
                    // Do not show buttons while waiting for refresh
                } else if is_installed {
                    //TODO: what if there are multiple desktop IDs?
                    if let Some(desktop_id) = selected.info.desktop_ids.first() {
                        buttons.push(
                            widget::button::suggested(fl!("open"))
                                .on_press(Message::OpenDesktopId(desktop_id.clone()))
                                .into(),
                        );
                    }
                    if let Some(update) = update_opt {
                        buttons.push(
                            widget::button::standard(fl!("update"))
                                .on_press(update)
                                .into(),
                        );
                    }
                    if selected.id != SYSTEM_ID {
                        buttons.push(
                            widget::button::destructive(fl!("uninstall"))
                                .on_press(Message::Operation(
                                    OperationKind::Uninstall,
                                    selected.backend_name,
                                    selected.id.clone(),
                                    selected.info.clone(),
                                ))
                                .into(),
                        );
                    }
                } else {
                    buttons.push(
                        widget::button::suggested(fl!("install"))
                            .on_press(Message::Operation(
                                OperationKind::Install,
                                selected.backend_name,
                                selected.id.clone(),
                                selected.info.clone(),
                            ))
                            .into(),
                    )
                }
                column = column.push(
                    widget::row::with_children(vec![
                        widget::icon::icon(selected.icon.clone())
                            .size(ICON_SIZE_DETAILS)
                            .into(),
                        widget::column::with_children(vec![
                            widget::text::title2(&selected.info.name).into(),
                            widget::text(&selected.info.summary).into(),
                            widget::vertical_space(Length::Fixed(space_s.into())).into(),
                            widget::row::with_children(buttons).spacing(space_xs).into(),
                        ])
                        .into(),
                    ])
                    .align_items(Alignment::Center)
                    .spacing(space_m),
                );
                //TODO: proper image scroller
                if let Some(screenshot) = selected.info.screenshots.get(selected.screenshot_shown) {
                    //TODO: get proper image dimensions
                    let image_height = Length::Fixed(480.0);
                    let mut row = widget::row::with_capacity(3).align_items(Alignment::Center);
                    {
                        let mut button = widget::button::icon(
                            widget::icon::from_name("go-previous-symbolic").size(16),
                        );
                        if selected.screenshot_shown > 0 {
                            button = button.on_press(Message::SelectedScreenshotShown(
                                selected.screenshot_shown - 1,
                            ));
                        }
                        row = row.push(button);
                    }
                    let image_element = if let Some(image) =
                        selected.screenshot_images.get(&selected.screenshot_shown)
                    {
                        widget::image(image.clone())
                            .width(Length::Fill)
                            .height(image_height)
                            .into()
                    } else {
                        widget::Space::new(Length::Fill, image_height).into()
                    };
                    row = row.push(
                        widget::column::with_children(vec![
                            image_element,
                            widget::text::caption(&screenshot.caption).into(),
                        ])
                        .align_items(Alignment::Center),
                    );
                    {
                        let mut button = widget::button::icon(
                            widget::icon::from_name("go-next-symbolic").size(16),
                        );
                        if selected.screenshot_shown + 1 < selected.info.screenshots.len() {
                            button = button.on_press(Message::SelectedScreenshotShown(
                                selected.screenshot_shown + 1,
                            ));
                        }
                        row = row.push(button);
                    }
                    column = column.push(row);
                }
                //TODO: parse markup in description
                column =
                    column.push(widget::text::body(&selected.info.description).width(Length::Fill));
                //TODO: description, releases, etc.
                widget::scrollable(column).into()
            }
            None => match &self.search_results {
                Some((input, results)) => {
                    //TODO: paging or dynamic load
                    let results_len = cmp::min(results.len(), 256);

                    let mut column = widget::column::with_capacity(1)
                        .padding([0, space_s])
                        .spacing(space_xxs)
                        .width(Length::Fill);
                    //TODO: back button?
                    if results.is_empty() {
                        column =
                            column.push(widget::text(fl!("no-results", search = input.as_str())));
                    } else {
                        column = column.align_items(Alignment::Center);
                    }
                    let mut flex_row = Vec::with_capacity(results_len);
                    for (result_i, result) in results.iter().take(results_len).enumerate() {
                        flex_row.push(
                            widget::mouse_area(result.card_view(&spacing))
                                .on_press(Message::SelectSearchResult(result_i))
                                .into(),
                        );
                    }
                    column = column.push(
                        widget::flex_row(flex_row)
                            .column_spacing(space_xxs)
                            .row_spacing(space_xxs),
                    );
                    widget::scrollable(column).into()
                }
                None => match self
                    .nav_model
                    .active_data::<NavPage>()
                    .map_or(NavPage::default(), |nav_page| *nav_page)
                {
                    NavPage::Explore => match self.explore_page_opt {
                        Some(explore_page) => {
                            let mut column = widget::column::with_capacity(2)
                                .padding([0, space_s])
                                .spacing(space_xxs)
                                .width(Length::Fill);
                            column = column.push(
                                widget::button::text(NavPage::Explore.title())
                                    .leading_icon(icon_cache_handle("go-previous-symbolic", 16))
                                    .on_press(Message::ExplorePage(None)),
                            );
                            column = column.push(widget::text::title4(explore_page.title()));
                            //TODO: ensure explore_page matches
                            match self.explore_results.get(&explore_page) {
                                Some(results) => {
                                    //TODO: paging or dynamic load
                                    let results_len = cmp::min(results.len(), 256);

                                    if results.is_empty() {
                                        //TODO: no results message?
                                    }
                                    let mut flex_row = Vec::with_capacity(results_len);
                                    for (result_i, result) in
                                        results.iter().take(results_len).enumerate()
                                    {
                                        flex_row.push(
                                            widget::mouse_area(result.card_view(&spacing))
                                                .on_press(Message::SelectExploreResult(
                                                    explore_page,
                                                    result_i,
                                                ))
                                                .into(),
                                        );
                                    }
                                    column = column.push(
                                        widget::flex_row(flex_row)
                                            .column_spacing(space_xxs)
                                            .row_spacing(space_xxs),
                                    );
                                }
                                None => {
                                    //TODO: loading message?
                                }
                            }
                            widget::scrollable(column).into()
                        }
                        None => {
                            let explore_pages = ExplorePage::all();
                            let mut column = widget::column::with_capacity(explore_pages.len() * 2)
                                .padding([0, space_s])
                                .spacing(space_xxs)
                                .width(Length::Fill);
                            for explore_page in explore_pages.iter() {
                                column = column.push(widget::row::with_children(vec![
                                    widget::text::title4(explore_page.title()).into(),
                                    widget::horizontal_space(Length::Fill).into(),
                                    widget::button::text(fl!("see-all"))
                                        .trailing_icon(icon_cache_handle("go-next-symbolic", 16))
                                        .on_press(Message::ExplorePage(Some(*explore_page)))
                                        .into(),
                                ]));
                                //TODO: ensure explore_page matches
                                match self.explore_results.get(&explore_page) {
                                    Some(results) => {
                                        let results_len = cmp::min(results.len(), 8);

                                        if results.is_empty() {
                                            //TODO: no results message?
                                        }
                                        let mut flex_row = Vec::with_capacity(results_len);
                                        for (result_i, result) in
                                            results.iter().take(results_len).enumerate()
                                        {
                                            flex_row.push(
                                                widget::mouse_area(result.card_view(&spacing))
                                                    .on_press(Message::SelectExploreResult(
                                                        *explore_page,
                                                        result_i,
                                                    ))
                                                    .into(),
                                            );
                                        }
                                        column = column.push(
                                            widget::flex_row(flex_row)
                                                .column_spacing(space_xxs)
                                                .row_spacing(space_xxs),
                                        );
                                    }
                                    None => {
                                        //TODO: loading message?
                                    }
                                }
                            }
                            widget::scrollable(column).into()
                        }
                    },
                    NavPage::Installed => {
                        let mut column = widget::column::with_capacity(3)
                            .padding([0, space_s])
                            .spacing(space_xxs)
                            .width(Length::Fill);
                        column = column.push(widget::text::title4(NavPage::Installed.title()));
                        match &self.installed {
                            Some(installed) => {
                                if installed.is_empty() {
                                    column =
                                        column.push(widget::text(fl!("no-installed-applications")));
                                }
                                let mut flex_row = Vec::with_capacity(installed.len());
                                for (installed_i, (_backend_name, package)) in
                                    installed.iter().enumerate()
                                {
                                    flex_row.push(
                                        widget::mouse_area(package.card_view(vec![], &spacing))
                                            .on_press(Message::SelectInstalled(installed_i))
                                            .into(),
                                    );
                                }
                                column = column.push(
                                    widget::flex_row(flex_row)
                                        .column_spacing(space_xxs)
                                        .row_spacing(space_xxs),
                                );
                            }
                            None => {
                                //TODO: loading message?
                            }
                        }
                        widget::scrollable(column).into()
                    }
                    //TODO: reduce duplication
                    NavPage::Updates => {
                        let mut column = widget::column::with_capacity(3)
                            .padding([0, space_s])
                            .spacing(space_xxs)
                            .width(Length::Fill);
                        column = column.push(widget::text::title4(NavPage::Updates.title()));
                        match &self.updates {
                            Some(updates) => {
                                if updates.is_empty() {
                                    column = column.push(widget::text(fl!("no-updates")));
                                } else {
                                    column = column.push(widget::row::with_children(vec![
                                        widget::button::standard(fl!("update-all"))
                                            .on_press(Message::UpdateAll)
                                            .into(),
                                        widget::horizontal_space(Length::Fill).into(),
                                    ]));
                                }
                                let mut flex_row = Vec::with_capacity(updates.len());
                                for (updates_i, (backend_name, package)) in
                                    updates.iter().enumerate()
                                {
                                    let mut waiting_refresh = false;
                                    for (other_backend_name, source_id, package_id) in self
                                        .waiting_installed
                                        .iter()
                                        .chain(self.waiting_updates.iter())
                                    {
                                        if other_backend_name == backend_name
                                            && source_id == &package.info.source_id
                                            && match_id(package_id, &package.id)
                                        {
                                            waiting_refresh = true;
                                            break;
                                        }
                                    }
                                    let mut progress_opt = None;
                                    for (_id, (op, progress)) in self.pending_operations.iter() {
                                        if &op.backend_name == backend_name
                                            && &op.info.source_id == &package.info.source_id
                                            && match_id(&op.package_id, &package.id)
                                        {
                                            progress_opt = Some(*progress);
                                            break;
                                        }
                                    }
                                    let controls = if let Some(progress) = progress_opt {
                                        vec![widget::progress_bar(0.0..=100.0, progress)
                                            .height(Length::Fixed(4.0))
                                            .into()]
                                    } else if waiting_refresh {
                                        vec![]
                                    } else {
                                        vec![widget::button::standard(fl!("update"))
                                            .on_press(Message::Operation(
                                                OperationKind::Update,
                                                backend_name,
                                                package.id.clone(),
                                                package.info.clone(),
                                            ))
                                            .into()]
                                    };
                                    flex_row.push(
                                        widget::mouse_area(package.card_view(controls, &spacing))
                                            .on_press(Message::SelectUpdates(updates_i))
                                            .into(),
                                    );
                                }
                                column = column.push(
                                    widget::flex_row(flex_row)
                                        .column_spacing(space_xxs)
                                        .row_spacing(space_xxs),
                                );
                            }
                            None => {
                                //TODO: loading message?
                            }
                        }
                        widget::scrollable(column).into()
                    }
                    //TODO: reduce duplication
                    nav_page => {
                        let mut column = widget::column::with_capacity(2)
                            .padding([0, space_s])
                            .spacing(space_xxs)
                            .width(Length::Fill);
                        column = column.push(widget::text::title4(nav_page.title()));
                        //TODO: ensure category matches?
                        match &self.category_results {
                            Some((_category, results)) => {
                                //TODO: paging or dynamic load
                                let results_len = cmp::min(results.len(), 256);

                                if results.is_empty() {
                                    //TODO: no results message?
                                }
                                let mut flex_row = Vec::with_capacity(results_len);
                                for (result_i, result) in
                                    results.iter().take(results_len).enumerate()
                                {
                                    flex_row.push(
                                        widget::mouse_area(result.card_view(&spacing))
                                            .on_press(Message::SelectCategoryResult(result_i))
                                            .into(),
                                    );
                                }
                                column = column.push(
                                    widget::flex_row(flex_row)
                                        .column_spacing(space_xxs)
                                        .row_spacing(space_xxs),
                                );
                            }
                            None => {
                                //TODO: loading message?
                            }
                        }
                        widget::scrollable(column).into()
                    }
                },
            },
        };

        // Uncomment to debug layout:
        //content.explain(cosmic::iced::Color::WHITE)
        content
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        struct ConfigSubscription;
        struct ThemeSubscription;

        let mut subscriptions = vec![
            event::listen_with(|event, _status| match event {
                Event::Keyboard(KeyEvent::KeyPressed { key, modifiers, .. }) => {
                    Some(Message::Key(modifiers, key))
                }
                _ => None,
            }),
            cosmic_config::config_subscription(
                TypeId::of::<ConfigSubscription>(),
                Self::APP_ID.into(),
                CONFIG_VERSION,
            )
            .map(|update| {
                if !update.errors.is_empty() {
                    log::debug!("errors loading config: {:?}", update.errors);
                }
                Message::SystemThemeModeChange(update.config)
            }),
            cosmic_config::config_subscription::<_, cosmic_theme::ThemeMode>(
                TypeId::of::<ThemeSubscription>(),
                cosmic_theme::THEME_MODE_ID.into(),
                cosmic_theme::ThemeMode::version(),
            )
            .map(|update| {
                if !update.errors.is_empty() {
                    log::debug!("errors loading theme mode: {:?}", update.errors);
                }
                Message::SystemThemeModeChange(update.config)
            }),
        ];

        for (id, (op, _)) in self.pending_operations.iter() {
            //TODO: use recipe?
            let id = *id;
            let backend_opt = self.backends.get(op.backend_name).map(|x| x.clone());
            let op = op.clone();
            subscriptions.push(subscription::channel(id, 16, move |msg_tx| async move {
                let msg_tx = Arc::new(tokio::sync::Mutex::new(msg_tx));
                let res = match backend_opt {
                    Some(backend) => {
                        let msg_tx = msg_tx.clone();
                        tokio::task::spawn_blocking(move || {
                            backend
                                .operation(
                                    op.kind,
                                    &op.package_id,
                                    &op.info,
                                    Box::new(move |progress| -> () {
                                        let _ = futures::executor::block_on(async {
                                            msg_tx
                                                .lock()
                                                .await
                                                .send(Message::PendingProgress(id, progress))
                                                .await
                                        });
                                    }),
                                )
                                .map_err(|err| err.to_string())
                        })
                        .await
                        .unwrap()
                    }
                    None => Err(format!("backend {:?} not found", op.backend_name)),
                };

                match res {
                    Ok(()) => {
                        let _ = msg_tx.lock().await.send(Message::PendingComplete(id)).await;
                    }
                    Err(err) => {
                        let _ = msg_tx
                            .lock()
                            .await
                            .send(Message::PendingError(id, err.to_string()))
                            .await;
                    }
                }

                loop {
                    tokio::time::sleep(time::Duration::new(1, 0)).await;
                }
            }));
        }

        if let Some(selected) = &self.selected_opt {
            for (screenshot_i, screenshot) in selected.info.screenshots.iter().enumerate() {
                let url = screenshot.url.clone();
                subscriptions.push(subscription::channel(
                    url.clone(),
                    16,
                    move |mut msg_tx| async move {
                        log::info!("fetch screenshot {}", url);
                        match reqwest::get(&url).await {
                            Ok(response) => match response.bytes().await {
                                Ok(bytes) => {
                                    log::info!(
                                        "fetched screenshot from {}: {} bytes",
                                        url,
                                        bytes.len()
                                    );
                                    let _ = msg_tx
                                        .send(Message::SelectedScreenshot(
                                            screenshot_i,
                                            url,
                                            bytes.to_vec(),
                                        ))
                                        .await;
                                }
                                Err(err) => {
                                    log::warn!("failed to read screenshot from {}: {}", url, err);
                                }
                            },
                            Err(err) => {
                                log::warn!("failed to request screenshot from {}: {}", url, err);
                            }
                        }
                        loop {
                            tokio::time::sleep(time::Duration::new(1, 0)).await;
                        }
                    },
                ));
            }
        }

        Subscription::batch(subscriptions)
    }
}
