// 应用管理器 — Rust + egui 实现的 Windows 已安装应用管理工具
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

const TILE_W: f32 = 104.0;
const TILE_H: f32 = 98.0;
const ICON_SIZE: f32 = 46.0;

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AppEntry {
    #[serde(default)]
    id: String,
    name: String,
    path: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    uninstall: String,
    #[serde(default)]
    install_location: String,
    category: Option<String>,
    #[serde(default)]
    manual: bool,
    #[serde(default)]
    hidden: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Config {
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    apps: Vec<AppEntry>,
    #[serde(default)]
    scan_source: String,
    #[serde(default)]
    font: Option<String>, // 应用界面字体文件路径（None = 默认微软雅黑）
}

#[derive(Clone, PartialEq, Debug, Default)]
enum View {
    #[default]
    All,
    Uncategorized,
    Cat(String),
    Removed,
}

struct NewCatDialog {
    name: String,
    app_to_move: Option<String>,
}

#[derive(Clone)]
struct FontItem {
    name: String, // 注册表显示名（已去类型后缀）
    path: String, // 字体文件完整路径
}

struct Counts {
    all: usize,
    uncategorized: usize,
    cats: HashMap<String, usize>,
    removed: usize,
}

struct AppManager {
    ctx: egui::Context,
    apps: Vec<AppEntry>,
    categories: Vec<String>,
    view: View,
    search: String,
    icons: HashMap<String, Option<egui::TextureHandle>>,
    status: String,
    config_path: PathBuf,
    new_cat: Option<NewCatDialog>,
    rename_cat: Option<(String, String)>, // (旧名, 输入缓冲)
    pending_inherit: Option<HashMap<String, String>>, // 数据源迁移时按名称继承分类
    scan_source: String,
    next_id: u64,
    font_path: Option<String>, // 当前应用字体文件
    font_dialog_open: bool,
    font_filter: String,
    font_list: Vec<FontItem>,
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() -> Result<(), eframe::Error> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1080.0, 720.0])
        .with_min_inner_size([720.0, 480.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "应用管理器",
        options,
        Box::new(|cc| Box::new(AppManager::new(cc))),
    )
}

/// 窗口 / 任务栏图标：内嵌的 assets/app.ico（256×256 帧），解码失败则用系统默认
fn window_icon() -> Option<egui::IconData> {
    const ICO: &[u8] = include_bytes!("../assets/app.ico");
    let img = image::load_from_memory(ICO).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some(egui::IconData {
        width: w,
        height: h,
        rgba: rgba.into_raw(),
    })
}

impl AppManager {
    fn new(cc: &eframe::CreationContext) -> Self {
        let ctx = cc.egui_ctx.clone();
        ctx.set_visuals(egui::Visuals::dark());

        let config_path = choose_config_path();
        let (apps, categories, scan_source, font) = match load_config(&config_path) {
            Some(c) => (c.apps, c.categories, c.scan_source, c.font),
            None => (Vec::new(), Vec::new(), String::new(), None),
        };
        setup_fonts(&ctx, font.as_deref());

        let mut app = Self {
            ctx,
            apps,
            categories,
            view: View::default(),
            search: String::new(),
            icons: HashMap::new(),
            status: String::new(),
            config_path,
            new_cat: None,
            rename_cat: None,
            pending_inherit: None,
            scan_source,
            next_id: 0,
            font_path: font,
            font_dialog_open: false,
            font_filter: String::new(),
            font_list: Vec::new(),
        };

        // 为旧配置中缺少 id 的条目补齐唯一 id（身份标识，path 可能为空不可作身份）
        app.next_id = app
            .apps
            .iter()
            .filter_map(|a| a.id.strip_prefix('a').and_then(|s| s.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        for i in 0..app.apps.len() {
            if app.apps[i].id.is_empty() {
                let id = app.alloc_id();
                app.apps[i].id = id;
            }
        }

        // 旧数据源（开始菜单扫描）迁移：保留手动应用与分类，自动条目按名称继承分类后重建
        let migrating = app.scan_source != SCAN_SRC;
        if migrating {
            let mut inherit: HashMap<String, String> = HashMap::new();
            app.apps.retain(|a| {
                if a.manual {
                    true
                } else {
                    if let Some(c) = &a.category {
                        inherit.insert(a.name.to_lowercase(), c.clone());
                    }
                    false
                }
            });
            app.pending_inherit = Some(inherit);
            app.scan_source = SCAN_SRC.to_string();
        }

        app.rescan();

        if migrating {
            app.status = format!(
                "已切换到「程序和功能」数据源，旧分类按名称继承。{}",
                app.status
            );
        }
        app
    }

    // -----------------------------------------------------------------------
    // 配置持久化
    // -----------------------------------------------------------------------

    fn alloc_id(&mut self) -> String {
        self.next_id += 1;
        format!("a{}", self.next_id)
    }

    fn save_config(&mut self) {
        let cfg = Config {
            categories: self.categories.clone(),
            apps: self.apps.clone(),
            scan_source: self.scan_source.clone(),
            font: self.font_path.clone(),
        };
        let Ok(s) = serde_json::to_string_pretty(&cfg) else {
            return;
        };
        if fs::write(&self.config_path, &s).is_err() {
            if let Ok(ad) = std::env::var("APPDATA") {
                let p = PathBuf::from(ad).join("app_manager_config.json");
                if fs::write(&p, &s).is_ok() {
                    self.config_path = p;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 扫描
    // -----------------------------------------------------------------------

    fn rescan(&mut self) {
        let found = scan_installed_apps();
        let mut added = 0usize;
        for s in found.iter() {
            let key = s.name.to_lowercase();
            // 已存在（含已隐藏）的同名自动条目不再重复添加
            if self
                .apps
                .iter()
                .any(|a| !a.manual && a.name.to_lowercase() == key)
            {
                continue;
            }
            let category = self
                .pending_inherit
                .as_mut()
                .and_then(|m| m.remove(&key));
            let id = self.alloc_id();
            self.apps.push(AppEntry {
                id,
                name: s.name.clone(),
                path: s.target.clone().unwrap_or_default(),
                icon: s.icon_src.clone().unwrap_or_default(),
                uninstall: s.uninstall.clone(),
                install_location: s.install_location.clone(),
                category,
                manual: false,
                hidden: false,
            });
            added += 1;
        }
        self.pending_inherit = None;

        // 已卸载（注册表条目消失）的应用自动移除
        let present: HashSet<String> = found
            .iter()
            .map(|s| s.name.to_lowercase())
            .collect();
        let before = self.apps.len();
        self.apps
            .retain(|a| a.manual || present.contains(&a.name.to_lowercase()));
        let removed = before - self.apps.len();
        self.save_config();
        let visible = self.apps.iter().filter(|a| !a.hidden).count();
        self.status = format!("扫描完成：新增 {added}，移除失效 {removed}，共 {visible} 个应用");
    }

    // -----------------------------------------------------------------------
    // 增删改
    // -----------------------------------------------------------------------

    fn add_manual_app(&mut self) {
        let Some(p) = rfd::FileDialog::new()
            .set_title("选择应用（exe / lnk）")
            .add_filter("应用程序", &["exe", "lnk"])
            .pick_file()
        else {
            return;
        };
        let path = p.to_string_lossy().to_string();
        let key = path.to_lowercase();
        if self.apps.iter().any(|a| a.path.to_lowercase() == key) {
            self.status = "⚠ 该应用已在列表中".to_string();
            return;
        }
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let category = match &self.view {
            View::Cat(c) => Some(c.clone()),
            _ => None,
        };
        self.status = format!("✅ 已添加「{name}」");
        let id = self.alloc_id();
        self.apps.push(AppEntry {
            id,
            name,
            path,
            icon: String::new(),
            uninstall: String::new(),
            install_location: String::new(),
            category,
            manual: true,
            hidden: false,
        });
        self.save_config();
    }

    fn set_category(&mut self, id: &str, cat: Option<String>) {
        if let Some(a) = self.apps.iter_mut().find(|a| a.id == id) {
            a.category = cat;
        }
        self.save_config();
    }

    fn assign_cat_by_id(&mut self, id: &str, cat: Option<String>) {
        if let Some(a) = self.apps.iter_mut().find(|a| a.id == id) {
            a.category = cat;
        }
    }

    fn remove_app(&mut self, id: &str) {
        let Some(pos) = self.apps.iter().position(|a| a.id == id) else {
            return;
        };
        let app = self.apps[pos].clone();
        self.apps[pos].hidden = true;
        self.status = format!(
            "🗑 「{}」已移入「已移除」，可从左侧栏打开该视图恢复",
            app.name
        );
        self.save_config();
    }

    fn restore_app(&mut self, id: &str) {
        if let Some(a) = self.apps.iter_mut().find(|a| a.id == id) {
            a.hidden = false;
            let name = a.name.clone();
            self.status = format!("已恢复「{name}」");
        }
        self.save_config();
    }

    /// 彻底删除（仅手动添加的应用；自动扫描的应用重新扫描会回来）
    fn purge_app(&mut self, id: &str) {
        let mut name = String::new();
        self.apps.retain(|a| {
            if a.id == id {
                name = a.name.clone();
                false
            } else {
                true
            }
        });
        if !name.is_empty() {
            self.status = format!("🗑 已彻底删除「{name}」");
            self.save_config();
        }
    }

    fn delete_category(&mut self, cat: &str) {
        self.categories.retain(|c| c != cat);
        for a in &mut self.apps {
            if a.category.as_deref() == Some(cat) {
                a.category = None;
            }
        }
        if self.view == View::Cat(cat.to_string()) {
            self.view = View::All;
        }
        self.status = format!("🗑 已删除分类「{cat}」，其中应用移入未分类");
        self.save_config();
    }

    // -----------------------------------------------------------------------
    // 查询
    // -----------------------------------------------------------------------

    fn counts(&self) -> Counts {
        let mut c = Counts {
            all: 0,
            uncategorized: 0,
            cats: HashMap::new(),
            removed: 0,
        };
        for a in &self.apps {
            if a.hidden {
                c.removed += 1;
                continue;
            }
            c.all += 1;
            match &a.category {
                None => c.uncategorized += 1,
                Some(k) => *c.cats.entry(k.clone()).or_default() += 1,
            }
        }
        c
    }

    fn filtered(&self) -> Vec<AppEntry> {
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<AppEntry> = self
            .apps
            .iter()
            .filter(|a| match &self.view {
                View::All => !a.hidden,
                View::Uncategorized => !a.hidden && a.category.is_none(),
                View::Cat(c) => !a.hidden && a.category.as_deref() == Some(c.as_str()),
                View::Removed => a.hidden,
            })
            .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q))
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        v
    }

    fn icon_for(&mut self, app: &AppEntry) -> Option<egui::TextureHandle> {
        let src = if app.path.is_empty() {
            app.icon.clone()
        } else {
            app.path.clone()
        };

        // 先取真实图标（带负缓存）；失败则回退到系统默认类型图标（按扩展名共享缓存）
        if !src.is_empty() {
            let key = src.to_lowercase();
            let cached = self.icons.get(&key).cloned();
            match cached {
                Some(Some(t)) => return Some(t),
                Some(None) => {} // 真实图标不可用 → 走类型默认
                None => {
                    let tex = unsafe { extract_icon_rgba(&src) }.map(|img| {
                        self.ctx.load_texture(
                            format!("icon:{key}"),
                            img,
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    self.icons.insert(key, tex.clone());
                    if let Some(t) = tex {
                        return Some(t);
                    }
                }
            }
        }

        let ext = file_ext_lower(&src).unwrap_or_else(|| "exe".to_string());
        let tkey = format!("type:{ext}");
        let cached = self.icons.get(&tkey).cloned();
        if let Some(t) = cached {
            return t;
        }
        let tex = unsafe { extract_default_icon_rgba(&ext) }.map(|img| {
            self.ctx
                .load_texture(format!("icon:{tkey}"), img, egui::TextureOptions::LINEAR)
        });
        self.icons.insert(tkey, tex.clone());
        tex
    }

    fn launch_app(&mut self, app: &AppEntry) {
        if app.path.is_empty() {
            self.status = format!("⚠ 「{}」未找到启动目标，可右键打开所在位置", app.name);
            return;
        }
        let ok = shell_exec(&app.path, None);
        self.status = if ok {
            format!("✅ 已启动「{}」", app.name)
        } else {
            format!("❌ 启动失败「{}」", app.name)
        };
    }

    // -----------------------------------------------------------------------
    // UI
    // -----------------------------------------------------------------------

    fn top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("应用管理器");
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("🔍 搜索应用…")
                        .desired_width(200.0),
                );
                if ui.button("➕ 添加应用").clicked() {
                    self.add_manual_app();
                }
                if ui.button("🔄 重新扫描").clicked() {
                    self.rescan();
                }
                if ui.button("🔤 字体").clicked() {
                    self.open_font_dialog();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !self.status.is_empty() {
                        ui.weak(&self.status);
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("categories")
            .resizable(true)
            .default_width(190.0)
            .min_width(150.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.heading("分类");
                ui.add_space(4.0);
                let counts = self.counts();
                ui.selectable_value(
                    &mut self.view,
                    View::All,
                    format!("🌐 全部 ({})", counts.all),
                );
                ui.selectable_value(
                    &mut self.view,
                    View::Uncategorized,
                    format!("📥 未分类 ({})", counts.uncategorized),
                );
                ui.selectable_value(
                    &mut self.view,
                    View::Removed,
                    format!("🗑 已移除 ({})", counts.removed),
                );
                ui.separator();
                for cat in self.categories.clone() {
                    let cnt = counts.cats.get(&cat).copied().unwrap_or(0);
                    let r = ui.selectable_value(
                        &mut self.view,
                        View::Cat(cat.clone()),
                        format!("📁 {cat} ({cnt})"),
                    );
                    r.context_menu(|ui| {
                        if ui.button("✏️ 重命名").clicked() {
                            self.rename_cat = Some((cat.clone(), cat.clone()));
                            ui.close_menu();
                        }
                        if ui.button("🗑 删除分类").clicked() {
                            self.delete_category(&cat);
                            ui.close_menu();
                        }
                    });
                }
                ui.separator();
                if ui.button("➕ 新建分类").clicked() {
                    self.new_cat = Some(NewCatDialog {
                        name: String::new(),
                        app_to_move: None,
                    });
                }
            });
    }

    fn central_panel(&mut self, ctx: &egui::Context) {
        let apps = self.filtered();
        egui::CentralPanel::default().show(ctx, |ui| {
            if apps.is_empty() {
                ui.add_space(80.0);
                ui.vertical_centered(|ui| {
                    ui.heading("没有应用");
                    ui.add_space(8.0);
                    if self.view == View::Removed {
                        ui.weak("从列表移除的应用会出现在这里；右键图标可恢复");
                    } else {
                        ui.weak("点击右上角「➕ 添加应用」手动添加，或「🔄 重新扫描」自动检测");
                    }
                });
                return;
            }
            egui::ScrollArea::both()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    let cols = ((ui.available_width() / (TILE_W + 6.0)) as usize).max(1);
                    for chunk in apps.chunks(cols) {
                        ui.horizontal(|ui| {
                            for app in chunk {
                                self.tile(ui, app);
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
        });
    }

    fn tile(&mut self, ui: &mut egui::Ui, app: &AppEntry) {
        let tex = self.icon_for(app);
        let resp = ui.allocate_response(
            egui::vec2(TILE_W, TILE_H),
            egui::Sense::click(),
        );
        let r = resp.rect;
        let painter = ui.painter();

        if resp.hovered() {
            painter.rect_filled(
                r.expand(2.0),
                8.0,
                egui::Color32::from_white_alpha(16),
            );
        }

        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(r.center().x, r.top() + 8.0 + ICON_SIZE * 0.5),
            egui::vec2(ICON_SIZE, ICON_SIZE),
        );
        match &tex {
            Some(t) => {
                painter.image(
                    t.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(icon_rect.shrink(6.0), 6.0, egui::Color32::from_gray(60));
                painter.text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "❔",
                    egui::FontId::proportional(16.0),
                    egui::Color32::from_gray(150),
                );
            }
        }

        let mut job = egui::text::LayoutJob::single_section(
            app.name.clone(),
            egui::TextFormat::simple(
                egui::FontId::proportional(12.5),
                egui::Color32::from_gray(225),
            ),
        );
        job.wrap.max_width = TILE_W - 8.0;
        job.wrap.max_rows = 2;
        job.wrap.overflow_character = Some('…');
        let galley = ui.fonts(|f| f.layout_job(job));
        let ty = r.top() + 8.0 + ICON_SIZE + 4.0;
        let tx = r.left() + ((TILE_W - galley.size().x) * 0.5).max(0.0);
        painter.galley(egui::pos2(tx, ty), galley, egui::Color32::WHITE);

        if resp.double_clicked() {
            self.launch_app(app);
        }

        self.app_context_menu(&resp, app);
    }

    fn app_context_menu(&mut self, resp: &egui::Response, app: &AppEntry) {
        if app.hidden {
            // 「已移除」视图中的条目：恢复 / 彻底删除
            resp.context_menu(|ui| {
                if ui.button("恢复").clicked() {
                    self.restore_app(&app.id);
                    ui.close_menu();
                }
                if app.manual && ui.button("🗑 彻底删除").clicked() {
                    self.purge_app(&app.id);
                    ui.close_menu();
                }
            });
            return;
        }
        resp.context_menu(|ui| {
            if ui.button("🚀 运行").clicked() {
                self.launch_app(app);
                ui.close_menu();
            }
            if ui.button("📂 打开所在位置").clicked() {
                // 优先：在资源管理器中定位并高亮启动目标文件本身
                let mut opened = false;
                if !app.path.is_empty() && !app.path.to_lowercase().starts_with("steam://") {
                    if Path::new(&app.path).is_file() {
                        opened = shell_exec(
                            "explorer.exe",
                            Some(&format!("/select,\"{}\"", app.path)),
                        );
                    }
                }
                // 回退链：安装目录 → 启动目标父目录 → 卸载程序所在目录
                if !opened {
                    let mut dir = String::new();
                    if !app.install_location.is_empty()
                        && Path::new(&app.install_location).is_dir()
                    {
                        dir = app.install_location.clone();
                    }
                    if dir.is_empty() && !app.path.is_empty() {
                        if let Some(p) = Path::new(&app.path).parent() {
                            if p.is_dir() {
                                dir = p.to_string_lossy().to_string();
                            }
                        }
                    }
                    if dir.is_empty() && !app.uninstall.is_empty() {
                        let (f, _) = parse_command_line(&app.uninstall);
                        if let Some(p) = Path::new(&f).parent() {
                            if p.is_dir() {
                                dir = p.to_string_lossy().to_string();
                            }
                        }
                    }
                    if dir.is_empty() {
                        self.status = format!("⚠ 「{}」没有可用的安装位置信息", app.name);
                    } else if !shell_exec(&dir, None) {
                        self.status = format!("❌ 无法打开「{dir}」");
                    }
                }
                ui.close_menu();
            }
            ui.separator();
            ui.menu_button("➡ 移动到分类", |ui| {
                let mark_unc = if app.category.is_none() { "√ " } else { "" };
                if ui.button(format!("{mark_unc}未分类")).clicked() {
                    self.set_category(&app.id, None);
                    ui.close_menu();
                }
                for c in self.categories.clone() {
                    let mark = if app.category.as_deref() == Some(c.as_str()) {
                        "√ "
                    } else {
                        ""
                    };
                    if ui.button(format!("{mark}{c}")).clicked() {
                        self.set_category(&app.id, Some(c));
                        ui.close_menu();
                    }
                }
            });
            if ui.button("🆕 新建分类并移入…").clicked() {
                self.new_cat = Some(NewCatDialog {
                    name: String::new(),
                    app_to_move: Some(app.id.clone()),
                });
                ui.close_menu();
            }
            ui.separator();
            if ui.button("🗑 从列表移除").clicked() {
                self.remove_app(&app.id);
                ui.close_menu();
            }
            if !app.uninstall.is_empty() {
                ui.separator();
                if ui.button("⚠ 卸载…").clicked() {
                    let (file, args) = parse_command_line(&app.uninstall);
                    let ok = shell_exec(&file, args.as_deref());
                    self.status = if ok {
                        format!("✅ 已启动「{}」的卸载程序", app.name)
                    } else {
                        format!("❌ 卸载程序启动失败「{}」", app.name)
                    };
                    ui.close_menu();
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // 新建分类
        if self.new_cat.is_some() {
            let mut d = self.new_cat.take().unwrap();
            let mut open = true;
            let mut close = false;
            egui::Window::new("新建分类")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(match d.app_to_move {
                        Some(_) => "输入分类名称，应用将移入该分类：",
                        None => "输入分类名称：",
                    });
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut d.name)
                            .desired_width(240.0)
                            .hint_text("例如：开发工具"),
                    );
                    let enter =
                        r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let confirm = ui.button("确定").clicked() || enter;
                        if confirm {
                            let name = d.name.trim().to_string();
                            if !name.is_empty() && !self.categories.contains(&name) {
                                self.categories.push(name.clone());
                            }
                            if let Some(p) = d.app_to_move.take() {
                                self.assign_cat_by_id(&p, Some(name));
                            }
                            self.save_config();
                            close = true;
                        }
                        if ui.button("取消").clicked() {
                            close = true;
                        }
                    });
                });
            if close || !open {
                self.new_cat = None;
            } else {
                self.new_cat = Some(d);
            }
        }

        // 重命名分类
        if self.rename_cat.is_some() {
            let mut st = self.rename_cat.take().unwrap();
            let mut open = true;
            let mut close = false;
            egui::Window::new("重命名分类")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("新的分类名称：");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut st.1).desired_width(240.0),
                    );
                    let enter =
                        r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let confirm = ui.button("确定").clicked() || enter;
                        if confirm {
                            let new_name = st.1.trim().to_string();
                            if !new_name.is_empty()
                                && new_name != st.0
                                && !self.categories.contains(&new_name)
                            {
                                let old = st.0.clone();
                                for c in &mut self.categories {
                                    if *c == old {
                                        *c = new_name.clone();
                                    }
                                }
                                for a in &mut self.apps {
                                    if a.category.as_deref() == Some(old.as_str()) {
                                        a.category = Some(new_name.clone());
                                    }
                                }
                                if self.view == View::Cat(old) {
                                    self.view = View::Cat(new_name.clone());
                                }
                                self.save_config();
                            }
                            close = true;
                        }
                        if ui.button("取消").clicked() {
                            close = true;
                        }
                    });
                });
            if close || !open {
                self.rename_cat = None;
            } else {
                self.rename_cat = Some(st);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 字体设置
    // -----------------------------------------------------------------------

    fn open_font_dialog(&mut self) {
        self.font_list = enumerate_fonts();
        self.font_dialog_open = true;
    }

    fn apply_font(&mut self, ctx: &egui::Context, path: Option<&str>, name: &str) {
        setup_fonts(ctx, path);
        self.font_path = path.map(|s| s.to_string());
        self.status = format!("🔤 字体已切换：{name}");
        self.save_config();
    }

    fn font_dialog(&mut self, ctx: &egui::Context) {
        if !self.font_dialog_open {
            return;
        }
        let mut open = true;
        egui::Window::new("字体设置")
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                ui.weak("点击字体立即应用到本界面（中文始终有回退字体，不会缺字）：");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("恢复默认").clicked() {
                        self.apply_font(ctx, None, "默认（微软雅黑）");
                    }
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.font_filter)
                            .hint_text("🔍 过滤…")
                            .desired_width(160.0),
                    );
                });
                ui.separator();
                let filter = self.font_filter.to_lowercase();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    let list = self.font_list.clone();
                    for f in list {
                        if !filter.is_empty() && !f.name.to_lowercase().contains(&filter) {
                            continue;
                        }
                        let selected = self.font_path.as_deref() == Some(f.path.as_str());
                        let label = if selected {
                            format!("√ {}", f.name)
                        } else {
                            f.name.clone()
                        };
                        if ui.selectable_label(selected, label).clicked() {
                            self.apply_font(ctx, Some(&f.path), &f.name);
                        }
                    }
                });
            });
        if !open {
            self.font_dialog_open = false;
        }
    }

}

impl eframe::App for AppManager {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.top_panel(ctx);
        self.side_panel(ctx);
        self.central_panel(ctx);
        self.dialogs(ctx);
        self.font_dialog(ctx);
    }
}

// ---------------------------------------------------------------------------
// 应用扫描：注册表 Uninstall 项（与「控制面板\程序\程序和功能」同源）
// ---------------------------------------------------------------------------

const SCAN_SRC: &str = "registry_v1";

struct ScannedApp {
    name: String,
    target: Option<String>,   // 可启动目标（exe 或 lnk）
    icon_src: Option<String>, // 图标来源文件
    uninstall: String,        // 卸载命令
    install_location: String, // 安装目录
}

fn scan_installed_apps() -> Vec<ScannedApp> {
    let entries = registry_uninstall_entries();
    let lnk_index = build_lnk_index();

    let mut out: Vec<ScannedApp> = Vec::new();
    for (name, e) in entries {
        // 解析启动目标：DisplayIcon(exe) → 安装目录中的主程序 → 开始菜单同名快捷方式
        let mut target: Option<String> = None;
        let mut icon_src: Option<String> = None;

        if let Some((p, is_exe)) = clean_display_icon(&e.display_icon) {
            let bad = is_exe && looks_like_uninstaller(&p);
            if is_exe && !bad {
                target = Some(p.clone());
            }
            if !bad {
                icon_src = Some(p);
            }
        }
        if target.is_none()
            && !e.install_location.is_empty()
            && e.install_location.chars().count() > 3
            && Path::new(&e.install_location).is_dir()
        {
            target = pick_main_exe(&e.install_location, &name);
        }
        if target.is_none() {
            target = find_lnk(&lnk_index, &name);
        }
        // Steam 游戏：从 steam://uninstall/<appid> 推导可启动的 steam://run/<appid>
        if target.is_none() {
            if let Some(appid) = steam_appid(&e.uninstall) {
                target = Some(format!("steam://run/{appid}"));
            }
        }
        if icon_src.is_none() {
            icon_src = target.clone();
        }

        out.push(ScannedApp {
            name,
            target,
            icon_src,
            uninstall: e.uninstall,
            install_location: e.install_location,
        });
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// 读取三个 Uninstall 注册表位置，同名条目合并、过滤系统组件与更新项
fn registry_uninstall_entries() -> Vec<(String, RegEntry)> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let mut out: Vec<(String, RegEntry)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    let paths = [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];
    for (hive, sub) in paths {
        let root = match RegKey::predef(hive).open_subkey(sub) {
            Ok(k) => k,
            Err(_) => continue,
        };
        for key_name in root.enum_keys().filter_map(Result::ok) {
            let key = match root.open_subkey(&key_name) {
                Ok(k) => k,
                Err(_) => continue,
            };
            let display: String = match key.get_value::<String, _>("DisplayName") {
                Ok(v) => v.trim().to_string(),
                Err(_) => continue,
            };
            if display.is_empty() {
                continue;
            }
            let sys_comp: u32 = key.get_value("SystemComponent").unwrap_or(0);
            if sys_comp != 0 {
                continue;
            }
            if is_update_entry(&display) {
                continue;
            }
            let get = |n: &str| -> String { key.get_value::<String, _>(n).unwrap_or_default() };
            let entry = RegEntry {
                display_icon: get("DisplayIcon"),
                install_location: clean_path(&expand_env(&get("InstallLocation"))),
                uninstall: get("UninstallString"),
            };
            let lkey = display.to_lowercase();
            match index.get(&lkey) {
                Some(&i) => {
                    // 同名条目（如 64/32 位安装器重复写入）：补全缺失字段
                    let (_, e) = &mut out[i];
                    if e.display_icon.is_empty() {
                        e.display_icon = entry.display_icon;
                    }
                    if e.install_location.is_empty() {
                        e.install_location = entry.install_location;
                    }
                    if e.uninstall.is_empty() {
                        e.uninstall = entry.uninstall;
                    }
                }
                None => {
                    index.insert(lkey, out.len());
                    out.push((display, entry));
                }
            }
        }
    }
    out
}

struct RegEntry {
    display_icon: String,
    install_location: String,
    uninstall: String,
}

/// 判断 exe 是否像卸载/安装辅助程序（Steam 会把 DisplayIcon 写成 uninstall.exe）
fn looks_like_uninstaller(p: &str) -> bool {
    let name = Path::new(p)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    [
        "uninstall", "uninst", "setup", "instal", "update", "updater", "patch",
        "repair", "unwise",
    ]
    .iter()
    .any(|b| name.contains(b))
}

/// 过滤 Windows 更新类条目（KB 编号 / 各种 Update 前缀）
fn is_update_entry(name: &str) -> bool {
    let l = name.to_lowercase();
    if l.len() > 2 && l.starts_with("kb") && l.as_bytes()[2].is_ascii_digit() {
        return true;
    }
    [
        "security update",
        "update for",
        "cumulative update",
        "definition update",
        "hotfix",
    ]
    .iter()
    .any(|p| l.starts_with(p))
}

/// 开始菜单快捷方式索引：小写文件名 → lnk 完整路径（公共目录优先，作启动目标回退）
fn build_lnk_index() -> HashMap<String, String> {
    let mut idx: HashMap<String, String> = HashMap::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(pd) = std::env::var("ProgramData") {
        dirs.push(PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(ad) = std::env::var("APPDATA") {
        dirs.push(PathBuf::from(ad).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    for dir in &dirs {
        if !dir.is_dir() {
            continue;
        }
        for entry in WalkDir::new(dir)
            .max_depth(8)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            if !p
                .extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("lnk"))
            {
                continue;
            }
            if let Some(stem) = p.file_stem() {
                let k = stem.to_string_lossy().to_lowercase();
                idx.entry(k)
                    .or_insert_with(|| p.to_string_lossy().to_string());
            }
        }
    }
    idx
}

/// 按显示名匹配开始菜单快捷方式：精确名 → 名称包含（取最长匹配）
fn find_lnk(idx: &HashMap<String, String>, display: &str) -> Option<String> {
    let dl = display.to_lowercase();
    if let Some(p) = idx.get(&dl) {
        return Some(p.clone());
    }
    let mut best: Option<(usize, String)> = None;
    for (stem, path) in idx {
        if stem.chars().count() < 4 || dl.chars().count() < 4 {
            continue;
        }
        if dl.contains(stem.as_str()) && best.as_ref().map_or(true, |(n, _)| stem.len() > *n) {
            best = Some((stem.len(), path.clone()));
        }
    }
    best.map(|(_, p)| p)
}

/// 在安装目录中挑选主程序 exe：优先名称与显示名匹配，排除卸载/安装辅助程序
fn pick_main_exe(dir: &str, display: &str) -> Option<String> {
    const BAD: &[&str] = &[
        "uninstall", "uninst", "setup", "instal", "updater", "update", "crash",
        "report", "helper", "eula", "redist", "vcredist", "patch", "repair",
        "config", "settings", "activation", "licen", "welcome",
    ];
    let tokens: Vec<String> = display
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 4)
        .map(|t| t.to_string())
        .collect();

    let mut best: Option<(f64, String)> = None;
    let consider = |p: &Path, best: &mut Option<(f64, String)>| {
        if !p
            .extension()
            .map_or(false, |x| x.eq_ignore_ascii_case("exe"))
        {
            return;
        }
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64;
        let mut score = (size / 1_048_576.0).min(100.0);
        if BAD.iter().any(|b| name.contains(b)) {
            score -= 10000.0;
        }
        if tokens.iter().any(|t| name.contains(t)) {
            score += 500.0;
        }
        if score > 0.0 && best.as_ref().map_or(true, |(s, _)| score > *s) {
            *best = Some((score, p.to_string_lossy().to_string()));
        }
    };

    let root = PathBuf::from(dir);
    // 第一轮：安装目录根目录；无结果再向下一层子目录
    if let Ok(rd) = fs::read_dir(&root) {
        for e in rd.filter_map(Result::ok) {
            consider(&e.path(), &mut best);
        }
    }
    if best.is_none() {
        for e in WalkDir::new(&root)
            .max_depth(2)
            .into_iter()
            .filter_map(Result::ok)
        {
            consider(e.path(), &mut best);
        }
    }
    best.map(|(_, p)| p)
}

/// 展开路径中的 %ENV% 变量
fn expand_env(s: &str) -> String {
    let parts: Vec<&str> = s.split('%').collect();
    if parts.len() < 3 {
        return s.to_string();
    }
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i % 2 == 1 {
            match std::env::var(p.trim()) {
                Ok(v) => out.push_str(&v),
                Err(_) => {
                    out.push('%');
                    out.push_str(p);
                    out.push('%');
                }
            }
        } else {
            out.push_str(p);
        }
    }
    out
}

/// 清理路径：去引号、去末尾分隔符、去空
fn clean_path(s: &str) -> String {
    let t = s.trim().trim_matches('"');
    if t.chars().count() > 3 {
        t.trim_end_matches(['\\', '/']).to_string()
    } else {
        t.to_string()
    }
}

/// 解析 DisplayIcon 值（如 `C:\x\app.exe,0` / `"C:\x\app.exe",0`）→（路径, 是否 exe）
fn clean_display_icon(s: &str) -> Option<(String, bool)> {
    let s = s.trim_matches('\0').trim();
    let part = if s.starts_with('"') {
        s.split('"').nth(1)?
    } else {
        s.split(',').next()?
    };
    let part = clean_path(part);
    if part.is_empty() || !Path::new(&part).is_file() {
        return None;
    }
    let is_exe = part.to_lowercase().ends_with(".exe");
    Some((part, is_exe))
}

/// 从卸载命令中提取 Steam AppID（如 `"D:\Steam\steam.exe" steam://uninstall/1203220`）
fn steam_appid(uninstall: &str) -> Option<String> {
    const P: &str = "steam://uninstall/";
    let idx = uninstall.find(P)?;
    let rest = &uninstall[idx + P.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// 解析卸载命令行 →（程序, 参数）
fn parse_command_line(s: &str) -> (String, Option<String>) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            let file = rest[..end].to_string();
            let args = rest[end + 1..].trim().to_string();
            return (file, if args.is_empty() { None } else { Some(args) });
        }
    }
    let lower = s.to_lowercase();
    if let Some(pos) = lower.rfind(".exe") {
        let cut = pos + 4;
        let file = s[..cut].trim_matches('"').to_string();
        let args = s[cut..].trim().to_string();
        return (file, if args.is_empty() { None } else { Some(args) });
    }
    (s.to_string(), None)
}

// ---------------------------------------------------------------------------
// Win32：图标提取与启动
// ---------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn shell_exec(file: &str, params: Option<&str>) -> bool {
    use winapi::um::shellapi::ShellExecuteW;
    use winapi::um::winuser::SW_SHOWNORMAL;

    let verb = wide("open");
    let file_w = wide(file);
    let params_w = params.map(wide);
    let params_ptr = params_w
        .as_ref()
        .map_or(std::ptr::null(), |v| v.as_ptr());
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file_w.as_ptr(),
            params_ptr,
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    (r as isize) > 32
}

/// 从文件提取图标并转为 RGBA（BGRA→RGBA，alpha 全零时视为不透明）
unsafe fn extract_icon_rgba(path: &str) -> Option<egui::ColorImage> {
    use winapi::um::shellapi::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};

    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

    let wpath = wide(path);
    let mut sfi: SHFILEINFOW = std::mem::zeroed();
    let ok = SHGetFileInfoW(
        wpath.as_ptr(),
        FILE_ATTRIBUTE_NORMAL,
        &mut sfi,
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_ICON | SHGFI_LARGEICON,
    );
    if ok == 0 || sfi.hIcon.is_null() {
        return None;
    }
    hicon_to_rgba(sfi.hIcon)
}

/// 按扩展名取系统默认类型图标（SHGFI_USEFILEATTRIBUTES，不要求文件存在）
unsafe fn extract_default_icon_rgba(ext: &str) -> Option<egui::ColorImage> {
    use winapi::um::shellapi::{
        SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES,
    };

    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

    let dummy = format!("placeholder.{ext}");
    let wpath = wide(&dummy);
    let mut sfi: SHFILEINFOW = std::mem::zeroed();
    let ok = SHGetFileInfoW(
        wpath.as_ptr(),
        FILE_ATTRIBUTE_NORMAL,
        &mut sfi,
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
    );
    if ok == 0 || sfi.hIcon.is_null() {
        return None;
    }
    hicon_to_rgba(sfi.hIcon)
}

/// 小写扩展名（不含点）
fn file_ext_lower(p: &str) -> Option<String> {
    Path::new(p)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
}

/// HICON → RGBA（BGRA→RGBA，alpha 全零视为不透明；负责销毁 hIcon）
unsafe fn hicon_to_rgba(hicon: winapi::shared::windef::HICON) -> Option<egui::ColorImage> {
    use winapi::um::winuser::{DestroyIcon, GetDC, GetIconInfo, ReleaseDC, ICONINFO};
    use winapi::um::wingdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };

    let mut ii: ICONINFO = std::mem::zeroed();
    if GetIconInfo(hicon, &mut ii) == 0 {
        DestroyIcon(hicon);
        return None;
    }
    let cleanup = || {
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor as _);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask as _);
        }
        DestroyIcon(hicon);
    };

    if ii.hbmColor.is_null() {
        cleanup();
        return None;
    }
    let mut bm: BITMAP = std::mem::zeroed();
    if GetObjectW(
        ii.hbmColor as _,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bm as *mut _ as *mut winapi::ctypes::c_void,
    ) == 0
    {
        cleanup();
        return None;
    }
    let w = bm.bmWidth.max(0) as usize;
    let h = bm.bmHeight.max(0) as usize;
    if w == 0 || h == 0 || w > 512 || h > 512 {
        cleanup();
        return None;
    }

    let screen = GetDC(std::ptr::null_mut());
    let mem = CreateCompatibleDC(screen);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w as i32;
    bi.bmiHeader.biHeight = -(h as i32);
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut buf = vec![0u8; w * h * 4];
    let lines = GetDIBits(
        mem,
        ii.hbmColor as _,
        0,
        h as u32,
        buf.as_mut_ptr() as *mut winapi::ctypes::c_void,
        &mut bi,
        DIB_RGB_COLORS,
    );
    DeleteDC(mem);
    ReleaseDC(std::ptr::null_mut(), screen);
    cleanup();
    if lines == 0 {
        return None;
    }

    let mut has_alpha = false;
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        if px[3] != 0 {
            has_alpha = true;
        }
    }
    if !has_alpha {
        for px in buf.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &buf))
}

// ---------------------------------------------------------------------------
// 字体与配置路径
// ---------------------------------------------------------------------------

/// 加载系统中文字体（微软雅黑等），否则 egui 默认字体无法渲染中文
fn setup_fonts(ctx: &egui::Context, preferred: Option<&str>) {
    let mut fonts = egui::FontDefinitions::default();
    // 用户设置的字体排在最前（拉丁与汉字均优先命中）
    let mut front = 0usize;
    if let Some(p) = preferred {
        if !p.is_empty() && Path::new(p).is_file() {
            if let Ok(data) = fs::read(p) {
                fonts
                    .font_data
                    .insert("user_font".to_string(), egui::FontData::from_owned(data).into());
                if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                    fam.insert(0, "user_font".to_string());
                    front = 1;
                }
            }
        }
    }
    // CJK 回退始终保留，任何选择下中文都不会缺字形
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\Deng.ttf",
    ];
    for (i, c) in candidates.iter().enumerate() {
        if let Ok(data) = fs::read(c) {
            let id = format!("cjk_{i}");
            fonts
                .font_data
                .insert(id.clone(), egui::FontData::from_owned(data).into());
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                fam.insert(front, id.clone());
            }
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                fam.push(id);
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

/// 枚举系统已安装字体（HKLM / HKCU Fonts 注册表项），同名条目取文件名较短者
fn enumerate_fonts() -> Vec<FontItem> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    let fonts_dir = PathBuf::from(r"C:\Windows\Fonts");
    let mut by_name: HashMap<String, String> = HashMap::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts",
            KEY_READ,
        ) else {
            continue;
        };
        for (name, val) in key.enum_values().flatten() {
            let file = val.to_string();
            let file = file.trim().trim_matches('"');
            if file.is_empty() {
                continue;
            }
            let path = if file.contains('\\') {
                PathBuf::from(file)
            } else {
                fonts_dir.join(file.split(',').next().unwrap_or(file))
            };
            if !path.is_file() {
                continue;
            }
            let path = path.to_string_lossy().to_string();
            // 显示名：去掉 " & ..." 连写与 " (TrueType)" 等类型后缀
            let mut display = name.split(" & ").next().unwrap_or(&name).to_string();
            if let Some(i) = display.find(" (") {
                display.truncate(i);
            }
            if display.is_empty() {
                continue;
            }
            let new_len = Path::new(&path).as_os_str().len();
            let replace = match by_name.get(&display) {
                None => true,
                Some(old) => Path::new(old).as_os_str().len() > new_len,
            };
            if replace {
                by_name.insert(display, path);
            }
        }
    }
    let mut out: Vec<FontItem> = by_name
        .into_iter()
        .map(|(name, path)| FontItem { name, path })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn choose_config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let probe = dir.join(".__am_probe");
            if fs::write(&probe, b"t").is_ok() {
                let _ = fs::remove_file(&probe);
                return dir.join("app_manager_config.json");
            }
        }
    }
    if let Ok(ad) = std::env::var("APPDATA") {
        return PathBuf::from(ad).join("app_manager_config.json");
    }
    PathBuf::from("app_manager_config.json")
}

fn load_config(path: &Path) -> Option<Config> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}
