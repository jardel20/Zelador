//! Carga assíncrona compartilhada: o trabalho bloqueante roda numa thread do `gio` e o
//! resultado volta à thread principal. Várias telas pedem o mesmo dado; ele é calculado uma vez.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;

type Waiter<T> = Box<dyn FnOnce(Rc<T>)>;

enum State<T> {
    Idle,
    Loading(Vec<Waiter<T>>),
    Ready(Rc<T>),
}

pub struct Shared<T: Send + 'static> {
    state: RefCell<State<T>>,
    load: Arc<dyn Fn() -> T + Send + Sync>,
}

impl<T: Send + 'static> Shared<T> {
    pub fn new(load: impl Fn() -> T + Send + Sync + 'static) -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(State::Idle),
            load: Arc::new(load),
        })
    }

    /// Chama `then` com o valor, assim que existir. O primeiro pedido dispara a carga.
    pub fn get(self: &Rc<Self>, then: impl FnOnce(Rc<T>) + 'static) {
        let start = {
            let mut state = self.state.borrow_mut();
            match &mut *state {
                State::Ready(value) => {
                    let value = Rc::clone(value);
                    drop(state);
                    then(value);
                    return;
                }
                State::Loading(waiters) => {
                    waiters.push(Box::new(then));
                    false
                }
                State::Idle => {
                    *state = State::Loading(vec![Box::new(then)]);
                    true
                }
            }
        };
        if start {
            self.start();
        }
    }

    /// Descarta o valor e recarrega, avisando quem pedir depois.
    pub fn refresh(self: &Rc<Self>, then: impl FnOnce(Rc<T>) + 'static) {
        let mut state = self.state.borrow_mut();
        match &mut *state {
            State::Loading(waiters) => {
                waiters.push(Box::new(then));
                return;
            }
            State::Idle | State::Ready(_) => *state = State::Loading(vec![Box::new(then)]),
        }
        drop(state);
        self.start();
    }

    fn start(self: &Rc<Self>) {
        let shared = Rc::clone(self);
        let load = Arc::clone(&self.load);
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || load()).await;
            let Ok(value) = result else {
                tracing::error!("a carga em segundo plano terminou com pânico");
                *shared.state.borrow_mut() = State::Idle;
                return;
            };
            let value = Rc::new(value);
            let waiters = match std::mem::replace(
                &mut *shared.state.borrow_mut(),
                State::Ready(Rc::clone(&value)),
            ) {
                State::Loading(waiters) => waiters,
                State::Idle | State::Ready(_) => Vec::new(),
            };
            for waiter in waiters {
                waiter(Rc::clone(&value));
            }
        });
    }
}

/// Roda `work` fora da thread principal e entrega o resultado a `then` na thread principal.
pub fn run_in_background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    then: impl FnOnce(T) + 'static,
) {
    glib::spawn_future_local(async move {
        match gtk::gio::spawn_blocking(work).await {
            Ok(value) => then(value),
            Err(_) => tracing::error!("a tarefa em segundo plano terminou com pânico"),
        }
    });
}

/// Roda `work` numa thread e entrega cada evento que ela enviar a `on_event`, na thread
/// principal, na ordem em que foram enviados. Termina quando `work` termina.
pub fn stream_in_background<T: Send + 'static>(
    work: impl FnOnce(async_channel::Sender<T>) + Send + 'static,
    on_event: impl Fn(T) + 'static,
) {
    let (tx, rx) = async_channel::unbounded::<T>();
    gtk::gio::spawn_blocking(move || work(tx));
    glib::spawn_future_local(async move {
        while let Ok(event) = rx.recv().await {
            on_event(event);
        }
    });
}

type Listener<T> = Box<dyn Fn(&T)>;

/// Barramento simples entre telas: quem publica não conhece quem assina.
pub struct Bus<T> {
    listeners: RefCell<Vec<Listener<T>>>,
}

impl<T> Default for Bus<T> {
    fn default() -> Self {
        Self {
            listeners: RefCell::new(Vec::new()),
        }
    }
}

impl<T> Bus<T> {
    pub fn subscribe(&self, listener: impl Fn(&T) + 'static) {
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    pub fn publish(&self, event: &T) {
        for listener in self.listeners.borrow().iter() {
            listener(event);
        }
    }
}
