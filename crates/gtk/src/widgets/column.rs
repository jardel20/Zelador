//! Coluna de conteúdo com largura máxima, alinhada à esquerda como nos mockups.
//!
//! O `AdwClamp` centraliza o conteúdo; os mockups o alinham à esquerda, então esta é a
//! única exceção de widget próprio: um `LayoutManager` de ~30 linhas.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    pub struct ColumnLayout {
        pub max_width: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColumnLayout {
        const NAME: &'static str = "ZeladorColumnLayout";
        type Type = super::ColumnLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for ColumnLayout {}

    impl LayoutManagerImpl for ColumnLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let max_width = self.max_width.get();
            let Some(child) = widget.first_child() else {
                return (0, 0, -1, -1);
            };
            match orientation {
                gtk::Orientation::Horizontal => {
                    let (minimum, _, _, _) = child.measure(orientation, -1);
                    // Natural = largura máxima: ocupa o espaço disponível até esse limite.
                    (minimum, max_width.max(minimum), -1, -1)
                }
                _ => {
                    let width = if for_size < 0 {
                        -1
                    } else {
                        for_size.min(max_width)
                    };
                    child.measure(orientation, width)
                }
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, baseline: i32) {
            if let Some(child) = widget.first_child() {
                child.allocate(width.min(self.max_width.get()), height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct ColumnLayout(ObjectSubclass<imp::ColumnLayout>) @extends gtk::LayoutManager;
}

impl ColumnLayout {
    fn new(max_width: i32) -> Self {
        let layout: Self = glib::Object::new();
        layout.imp().max_width.set(max_width);
        layout
    }
}

pub fn left_column(max_width: i32, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    holder.append(child);
    holder.set_layout_manager(Some(ColumnLayout::new(max_width)));
    holder
}
