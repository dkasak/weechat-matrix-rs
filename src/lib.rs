#![feature(async_closure)]

mod commands;
mod config;
mod debug;
mod render;
mod room_buffer;
mod server;

use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;

use weechat::{
    buffer::{Buffer, BufferHandle},
    hooks::{BarItem, BarItemCallback, SignalCallback, SignalData, SignalHook},
    weechat_plugin, Args, Plugin, ReturnCode, Weechat,
};

use crate::commands::Commands;
use crate::config::ConfigHandle;
use crate::server::MatrixServer;

const PLUGIN_NAME: &str = "matrix";

#[derive(Clone, Debug)]
pub struct Servers(Rc<RefCell<HashMap<String, MatrixServer>>>);

impl Servers {
    fn new() -> Self {
        Servers(Rc::new(RefCell::new(HashMap::new())))
    }

    fn borrow(&self) -> Ref<'_, HashMap<String, MatrixServer>> {
        self.0.borrow()
    }

    fn borrow_mut(&self) -> RefMut<'_, HashMap<String, MatrixServer>> {
        self.0.borrow_mut()
    }
}

impl SignalCallback for Servers {
    fn callback(
        &mut self,
        _: &Weechat,
        _signal_name: &str,
        data: Option<SignalData>,
    ) -> ReturnCode {
        if let Some(data) = data {
            if let SignalData::Buffer(buffer) = data {
                let servers = self.borrow();

                for server in servers.values() {
                    for room_buffer in server.inner().room_buffers.values() {
                        if buffer == room_buffer.weechat_buffer() {
                            room_buffer.update_typing_notice();

                            return ReturnCode::Ok;
                        }
                    }
                }
            }
        }
        ReturnCode::Ok
    }
}

struct Matrix {
    servers: Servers,
    #[used]
    commands: Commands,
    #[used]
    config: ConfigHandle,
    #[used]
    status_bar: BarItem,
    #[used]
    typing_notice_signal: SignalHook,
    debug_buffer: RefCell<Option<BufferHandle>>,
}

impl std::fmt::Debug for Matrix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut fmt = f.debug_struct("Matrix");
        fmt.field("servers", &self.servers).finish()
    }
}

impl Matrix {
    fn autoconnect(servers: &mut HashMap<String, MatrixServer>) {
        for server in servers.values_mut() {
            if server.autoconnect() {
                match server.connect() {
                    Ok(_) => (),
                    Err(e) => Weechat::print(&format!("{:?}", e)),
                }
            }
        }
    }

    fn create_default_server(
        servers: &mut HashMap<String, MatrixServer>,
        config: &ConfigHandle,
    ) {
        // TODO change this to matrix.org.
        let server_name = "localhost";

        let mut config_borrow = config.borrow_mut();
        let mut section = config_borrow
            .search_section_mut("server")
            .expect("Can't get server section");

        let server = MatrixServer::new(server_name, config, &mut section);
        servers.insert(server_name.to_owned(), server);
    }
}

impl BarItemCallback for Servers {
    fn callback(&mut self, _: &Weechat, buffer: &Buffer) -> String {
        let servers = self.borrow();

        for server in servers.values() {
            let server = server.inner();

            for room in server.room_buffers().values() {
                let room_buffer = room.weechat_buffer();

                if &room_buffer == buffer && room.room().is_encrypted() {
                    return server.config().look().encrypted_room_sign();
                }
            }
        }

        "".to_owned()
    }
}

impl Plugin for Matrix {
    fn init(_: &Weechat, _args: Args) -> Result<Self, ()> {
        let servers = Servers::new();
        let config = ConfigHandle::new(&servers);
        let commands = Commands::hook_all(&servers, &config)?;

        // TODO move the bar creation into a separate file.
        let status_bar = BarItem::new("matrix_modes", servers.clone())?;

        tracing_subscriber::fmt()
            .with_writer(debug::Debug)
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();

        {
            let config_borrow = config.borrow();
            if config_borrow.read().is_err() {
                return Err(());
            }
        }

        {
            let mut servers_borrow = servers.borrow_mut();
            if servers_borrow.is_empty() {
                Matrix::create_default_server(&mut servers_borrow, &config)
            }
        }

        let typing = SignalHook::new("input_text_changed", servers.clone())
            .expect("Can't create signal hook for the typing notice cb");

        let plugin = Matrix {
            servers: servers.clone(),
            commands,
            config,
            status_bar,
            debug_buffer: RefCell::new(None),
            typing_notice_signal: typing,
        };

        Weechat::spawn(async move {
            let mut servers = servers.borrow_mut();
            Matrix::autoconnect(&mut servers);
        });

        Ok(plugin)
    }
}

impl Drop for Matrix {
    fn drop(&mut self) {
        let mut servers = self.servers.borrow_mut();

        // Buffer close callbacks get called after this, so disconnect here so
        // we don't leave all our rooms.
        //
        // TODO set a flag on the server as well so we don't even try to leave
        // the rooms, once leaving the rooms is implemented when the buffer gets
        // closed.
        for server in servers.values_mut() {
            server.disconnect();
        }
    }
}

weechat_plugin!(
    Matrix,
    name: "matrix",
    author: "Damir Jelić <poljar@termina.org.uk>",
    description: "Matrix protocol",
    version: "0.1.0",
    license: "ISC"
);
