//! "Espaço em disco": barra segmentada do /nix/store, Flatpak e maiores pacotes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use zelador_core::backends::NixBackend;
use zelador_core::space::{
    ClosureSets, PackageSize, SpaceBreakdown, SpaceInputs, breakdown, compute_sets, root_paths,
    top_packages,
};
use zelador_core::status::SystemStatus;
use zelador_core::store::store_name;
use zelador_core::units::format_bytes;

use super::Screen;
use crate::ctx::{Ctx, nix};
use crate::i18n::tr;
use crate::loader::run_in_background;
use crate::pages::Page;
use crate::widgets::rows;

const BAR_HEIGHT: i32 = 14;
const BAR_GAP: f64 = 2.0;
const TOP_PACKAGES: usize = 10;

/// Tokens de cor dos seis segmentos, na ordem da barra e da legenda.
const IN_USE_TOKENS: [&str; 3] = ["zel_use_1", "zel_use_2", "zel_use_3"];
const FREE_TOKENS: [&str; 3] = ["zel_free_1", "zel_free_2", "zel_free_3"];

type Sizes = Rc<Result<HashMap<String, u64>, String>>;
type Dead = Rc<Result<HashSet<String>, String>>;

struct View {
    ctx: Rc<Ctx>,
    intro: gtk::Label,
    loading: gtk::Box,
    bar: gtk::DrawingArea,
    segments: Rc<RefCell<Vec<(&'static str, f64)>>>,
    legend: gtk::Box,
    open: gtk::Button,
    outside: adw::PreferencesGroup,
    outside_rows: RefCell<Vec<gtk::Widget>>,
    packages: adw::PreferencesGroup,
    packages_rows: RefCell<Vec<gtk::Widget>>,
    /// Soma para a frase de abertura: o Flatpak chega por outro caminho.
    totals: RefCell<Totals>,
}

#[derive(Default)]
struct Totals {
    space: Option<SpaceBreakdown>,
    flatpak: Option<u64>,
}

pub fn build(ctx: &Rc<Ctx>, intro: &gtk::Label) -> Screen {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(32)
        .margin_top(22)
        .build();

    let loading = gtk::Box::builder().spacing(12).build();
    loading.append(&rows::spinner());
    loading.append(
        &gtk::Label::builder()
            .label(tr("Calculando o uso do /nix/store…"))
            .css_classes(["caption-text"])
            .build(),
    );

    let segments: Rc<RefCell<Vec<(&'static str, f64)>>> = Rc::default();
    let bar = gtk::DrawingArea::builder()
        .content_height(BAR_HEIGHT)
        .hexpand(true)
        .visible(false)
        .build();
    bar.set_accessible_role(gtk::AccessibleRole::Img);
    bar.update_property(&[gtk::accessible::Property::Label(&tr(
        "Uso do /nix/store por categoria",
    ))]);
    {
        let segments = Rc::clone(&segments);
        bar.set_draw_func(move |area, cr, width, height| {
            draw_bar(area, cr, width, height, &segments.borrow());
        });
    }
    let legend = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .margin_top(20)
        .visible(false)
        .build();
    let open = gtk::Button::builder()
        .label(tr("Liberar espaço"))
        .halign(gtk::Align::Start)
        .margin_top(16)
        .visible(false)
        .build();
    open.connect_clicked(glib::clone!(
        #[strong]
        ctx,
        move |_| ctx.go_to(Page::LiberarEspaco)
    ));

    let overview = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    overview.append(&loading);
    overview.append(&bar);
    overview.append(&legend);
    overview.append(&open);
    root.append(&overview);

    let outside = rows::group(&tr("Fora do Nix store"));
    let outside_placeholder = rows::muted_row(&tr("Lendo o Flatpak…"));
    outside.add(&outside_placeholder);
    root.append(&outside);

    let packages = rows::group(&tr("Maiores pacotes do sistema"));
    let packages_placeholder = rows::row(&tr("Calculando os maiores pacotes…"), None, false);
    packages_placeholder.add_suffix(&rows::spinner());
    packages.add(&packages_placeholder);
    root.append(&packages);

    let view = Rc::new(View {
        ctx: Rc::clone(ctx),
        intro: intro.clone(),
        loading,
        bar,
        segments,
        legend,
        open,
        outside,
        outside_rows: RefCell::new(vec![outside_placeholder.upcast()]),
        packages,
        packages_rows: RefCell::new(vec![packages_placeholder.upcast()]),
        totals: RefCell::new(Totals::default()),
    });
    start_loading(&view);

    Screen {
        body: root.upcast(),
        header_end: None,
    }
}

fn start_loading(view: &Rc<View>) {
    // O lixo e os tamanhos levam dezenas de segundos: pede os dois já, em paralelo.
    view.ctx.dead.get(|_| {});
    view.ctx.sizes.get(|_| {});

    let v = Rc::clone(view);
    view.ctx.overview.get(move |status| {
        v.show_flatpak(&status);
        let paths = root_paths(&v.ctx.env, &status);
        let v2 = Rc::clone(&v);
        run_in_background(
            move || compute_sets(&nix(), &paths).map_err(|e| e.to_string()),
            move |sets| {
                let v3 = Rc::clone(&v2);
                v2.ctx.sizes.get(move |sizes| {
                    let v4 = Rc::clone(&v3);
                    v3.ctx
                        .dead
                        .get(move |dead| v4.show_space(&sizes, &dead, &sets));
                });
            },
        );
    });

    let env = view.ctx.env.clone();
    let v = Rc::clone(view);
    run_in_background(
        move || top_packages(&nix(), &env, TOP_PACKAGES).map_err(|e| e.to_string()),
        move |result| v.show_packages(&result),
    );
}

impl View {
    fn show_flatpak(&self, status: &SystemStatus) {
        let mut rows_held = self.outside_rows.borrow_mut();
        rows::clear_group(&self.outside, &mut rows_held);
        let row = match &status.flatpak {
            Some(flatpak) => {
                self.totals.borrow_mut().flatpak = Some(flatpak.total_bytes);
                let unused = if flatpak.unused.is_empty() {
                    tr("Nenhum runtime sem uso")
                } else {
                    let plus = if flatpak.unused_bytes_partial {
                        "+"
                    } else {
                        ""
                    };
                    format!(
                        "{}{plus} em {} runtimes que nenhum app usa",
                        format_bytes(flatpak.unused_bytes),
                        flatpak.unused.len()
                    )
                };
                let row = rows::row("Flatpak", Some(&unused), false);
                row.add_suffix(&rows::suffix_label(&format_bytes(flatpak.total_bytes), &[]));
                row
            }
            None => rows::muted_row(&tr("Flatpak não encontrado")),
        };
        self.outside.add(&row);
        rows_held.push(row.upcast());
        drop(rows_held);
        self.refresh_intro();
    }

    fn show_space(&self, sizes: &Sizes, dead: &Dead, sets: &Result<ClosureSets, String>) {
        self.loading.set_visible(false);
        let (Ok(sizes), Ok(dead), Ok(sets)) = (&**sizes, &**dead, sets) else {
            let error = match (&**sizes, &**dead, sets) {
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => e.clone(),
                _ => String::new(),
            };
            self.intro.set_label(&format!(
                "{} {error}",
                tr("Não foi possível calcular o uso do store:")
            ));
            return;
        };
        let result = breakdown(&SpaceInputs {
            sizes,
            dead,
            system: &sets.system,
            home_manager: &sets.home_manager,
            old_generations: &sets.old_generations,
            forgotten_roots: &sets.forgotten_roots,
        });
        self.totals.borrow_mut().space = Some(result);

        let parts = [
            (IN_USE_TOKENS[0], result.system),
            (IN_USE_TOKENS[1], result.home_manager),
            (IN_USE_TOKENS[2], result.dev_environments),
            (FREE_TOKENS[0], result.old_generations),
            (FREE_TOKENS[1], result.forgotten_roots),
            (FREE_TOKENS[2], result.unreferenced),
        ];
        *self.segments.borrow_mut() = parts
            .iter()
            .map(|(token, bytes)| (*token, *bytes as f64 / result.total.max(1) as f64))
            .collect();
        self.bar.set_visible(true);
        self.bar.queue_draw();
        self.build_legend(&result);
        self.legend.set_visible(true);
        self.open.set_visible(true);
        self.refresh_intro();
    }

    fn build_legend(&self, result: &SpaceBreakdown) {
        while let Some(child) = self.legend.first_child() {
            self.legend.remove(&child);
        }
        let columns = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(2)
            .column_spacing(20)
            .row_spacing(20)
            .homogeneous(true)
            .build();
        columns.append(&legend_column(
            &format!("{} · {}", tr("Em uso"), format_bytes(result.in_use())),
            &[
                (IN_USE_TOKENS[0], tr("Sistema atual"), result.system),
                (
                    IN_USE_TOKENS[1],
                    tr("Home Manager atual"),
                    result.home_manager,
                ),
                (
                    IN_USE_TOKENS[2],
                    tr("Ambientes de desenvolvimento"),
                    result.dev_environments,
                ),
            ],
        ));
        columns.append(&legend_column(
            &format!(
                "{} · {}",
                tr("Pode sair"),
                format_bytes(result.reclaimable())
            ),
            &[
                (
                    FREE_TOKENS[0],
                    tr("Gerações antigas"),
                    result.old_generations,
                ),
                (
                    FREE_TOKENS[1],
                    tr("Links esquecidos"),
                    result.forgotten_roots,
                ),
                (FREE_TOKENS[2], tr("Sem referência"), result.unreferenced),
            ],
        ));
        self.legend.append(&columns);
        self.legend.append(
            &gtk::Label::builder()
                .label(tr(
                    "Estimativa: o Nix não informa qual raiz mantém cada caminho vivo.",
                ))
                .css_classes(["caption-text"])
                .xalign(0.0)
                .wrap(true)
                .build(),
        );
    }

    fn refresh_intro(&self) {
        let totals = self.totals.borrow();
        let Some(space) = totals.space else { return };
        let mut text = format!(
            "O /nix/store ocupa {}, e cerca de {} podem sair.",
            format_bytes(space.total),
            format_bytes(space.reclaimable())
        );
        if let Some(flatpak) = totals.flatpak {
            text.push_str(&format!(
                " Fora dele, o Flatpak ocupa mais {}.",
                format_bytes(flatpak)
            ));
        }
        self.intro.set_label(&text);
    }

    fn show_packages(self: &Rc<Self>, result: &Result<Vec<PackageSize>, String>) {
        let mut held = self.packages_rows.borrow_mut();
        rows::clear_group(&self.packages, &mut held);
        match result {
            Ok(packages) if !packages.is_empty() => {
                for package in packages {
                    let row = rows::row(&package.name, None, false);
                    row.add_css_class("mono-title");
                    row.add_suffix(&rows::suffix_label(
                        &format_bytes(package.closure_bytes),
                        &[],
                    ));
                    let why = gtk::Button::builder()
                        .child(&why_content())
                        .css_classes(["flat", "link-button"])
                        .valign(gtk::Align::Center)
                        .build();
                    let (view, package) = (Rc::clone(self), package.clone());
                    why.connect_clicked(move |_| view.show_why(&package));
                    row.add_suffix(&why);
                    self.packages.add(&row);
                    held.push(row.upcast());
                }
                // Só para verificação visual (ver `crate::dev_action`).
                if crate::dev_action().as_deref() == Some("why")
                    && let Some(first) = packages.first()
                {
                    self.show_why(first);
                }
            }
            Ok(_) => {
                let row = rows::muted_row(&tr("Nenhum pacote encontrado."));
                self.packages.add(&row);
                held.push(row.upcast());
            }
            Err(error) => {
                let row = rows::muted_row(&format!(
                    "{} {error}",
                    tr("Não foi possível listar os pacotes:")
                ));
                self.packages.add(&row);
                held.push(row.upcast());
            }
        }
    }

    /// "Por quê?": a cadeia de dependência do sistema até o pacote.
    fn show_why(&self, package: &PackageSize) {
        let dialog = adw::Dialog::builder()
            .title(format!(
                "{} {}?",
                tr("Por que está no sistema:"),
                package.name
            ))
            .content_width(520)
            .content_height(420)
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.append(
            &gtk::ListBoxRow::builder()
                .child(
                    &gtk::Label::builder()
                        .label(tr("Procurando a cadeia de dependência…"))
                        .css_classes(["caption-text"])
                        .margin_top(18)
                        .margin_bottom(18)
                        .build(),
                )
                .activatable(false)
                .build(),
        );
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .margin_start(18)
            .margin_end(18)
            .margin_bottom(18)
            .build();
        toolbar.set_content(Some(&scroll));
        dialog.set_child(Some(&toolbar));
        dialog.present(self.ctx.window().as_ref());

        let from = fs::canonicalize(&self.ctx.env.current_system)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| self.ctx.env.current_system.to_string_lossy().into_owned());
        let to = package.path.clone();
        run_in_background(
            move || nix().why_depends(&from, &to).map_err(|e| e.to_string()),
            move |result| {
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
                match result {
                    Ok(chain) if !chain.is_empty() => {
                        for (depth, path) in chain.iter().enumerate() {
                            let row = rows::row(store_name(path), None, false);
                            row.add_css_class("mono-title");
                            row.add_prefix(
                                &gtk::Label::builder()
                                    .label(format!("{}", depth + 1))
                                    .css_classes(["caption-text"])
                                    .build(),
                            );
                            list.append(&row);
                        }
                    }
                    Ok(_) => list.append(&rows::muted_row(&tr("Nenhuma cadeia encontrada."))),
                    Err(error) => list.append(&rows::muted_row(&error)),
                }
            },
        );
    }
}

fn why_content() -> gtk::Box {
    let content = gtk::Box::builder().spacing(4).build();
    content.append(&gtk::Label::new(Some(&tr("Por quê?"))));
    content.append(
        &gtk::Image::builder()
            .icon_name("caret-right-symbolic")
            .pixel_size(16)
            .build(),
    );
    content
}

/// Coluna da legenda: título e uma lista em cartão com cor, nome e tamanho.
fn legend_column(title: &str, items: &[(&str, String, u64)]) -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    column.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["section-title"])
            .xalign(0.0)
            .build(),
    );
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for (token, name, bytes) in items {
        let content = gtk::Box::builder()
            .spacing(12)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(18)
            .margin_end(18)
            .build();
        let swatch = gtk::Box::builder()
            .width_request(12)
            .height_request(12)
            .valign(gtk::Align::Center)
            .css_classes(["sw", &format!("sw-{}", token.trim_start_matches("zel_"))])
            .build();
        content.append(&swatch);
        content.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .build(),
        );
        content.append(&gtk::Label::new(Some(&format_bytes(*bytes))));
        list.append(
            &gtk::ListBoxRow::builder()
                .child(&content)
                .activatable(false)
                .build(),
        );
    }
    column.append(&list);
    column
}

/// Desenha a barra: cantos arredondados nas pontas e 2 px entre os segmentos.
fn draw_bar(
    area: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    segments: &[(&'static str, f64)],
) {
    let (w, h) = (f64::from(width), f64::from(height));
    let radius = h / 2.0;
    rounded_rect(cr, 0.0, 0.0, w, h, radius);
    cr.clip();
    let gaps = BAR_GAP
        * segments
            .iter()
            .filter(|(_, f)| *f > 0.0)
            .count()
            .saturating_sub(1) as f64;
    let usable = (w - gaps).max(0.0);
    let mut x = 0.0;
    for (token, fraction) in segments.iter().filter(|(_, f)| *f > 0.0) {
        let segment = (usable * fraction).max(1.0);
        let color = token_color(area, token);
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        cr.rectangle(x, 0.0, segment, h);
        if cr.fill().is_err() {
            return;
        }
        x += segment + BAR_GAP;
    }
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}

/// Cor de um token (`@define-color`) no tema atual, claro, escuro ou do sistema.
#[allow(deprecated)] // `lookup_color` é a única leitura de cor nomeada; o GTK ainda a mantém.
fn token_color(widget: &impl IsA<gtk::Widget>, name: &str) -> gtk::gdk::RGBA {
    widget
        .style_context()
        .lookup_color(name)
        .unwrap_or_else(|| gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0))
}
