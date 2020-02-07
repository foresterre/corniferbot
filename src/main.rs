#![warn(rust_2018_idioms)]

use anyhow::anyhow;
use std::fs::File;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use telegram_bot::{
    Api, CanSendMessage, Integer, Message, MessageKind, SendMessage, UpdateKind, UserId,
};
use tokio::stream::StreamExt;
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;

pub type BotResult<T> = anyhow::Result<T>;

fn init_logger() -> BotResult<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(())
}

#[tokio::main]
async fn main() -> BotResult<()> {
    init_logger()?;

    let token = env!("TG_TOKEN");

    let api = Api::new(token);

    // Our "database" (text file <3) will store all user ids, so we known who has subscribed to
    // updates from our bot. Here we open the database, or create it if it doesn't exist yet.
    let database = Database::open("database.txt")?;
    let database = Arc::new(database);

    let mut stream = api.stream();

    info!("Listening to stream");

    while let Some(item) = stream.next().await {
        let item = item?;

        // listen for commands send by a user
        if let UpdateKind::Message(message) = item.kind {
            process_message(api.clone(), message, database.clone()).await?;
        }

        // TODO remove, put it in its own loop, here just to show that send message works
        send_heartbeat(api.clone(), database.clone()).await?;
    }

    Ok(())
}

// todo: run processes next to each other
async fn _command_process(api: Api, database: Arc<Database>) -> BotResult<()> {
    let mut stream = api.stream();

    while let Some(item) = stream.next().await {
        let item = item?;

        // listen for commands send by a user
        if let UpdateKind::Message(message) = item.kind {
            process_message(api.clone(), message, database.clone()).await?;
        }

        send_heartbeat(api.clone(), database.clone()).await?;
    }

    Ok(())
}

// todo: run processes next to each other
async fn _broadcasting_process(api: Api, database: Arc<Database>) -> BotResult<()> {
    if let Err(e) = send_heartbeat(api.clone(), database).await {
        error!("heartbeat errored with: {}", e);
    }

    Ok(())
}

async fn process_message(api: Api, message: Message, database: Arc<Database>) -> BotResult<()> {
    match message.kind {
        MessageKind::Text { ref data, .. } => {
            let cmd = data.as_str();

            debug!("Received command: {}", cmd);

            match cmd {
                "/ping" => ping(api, message).await?,
                "/register" => register(api, message, &database).await?,
                _ => warn!("Command '{}' not supported", cmd),
            }
        }
        _ => warn!("Received unsupported message"),
    }

    Ok(())
}

async fn ping(api: Api, message: Message) -> BotResult<()> {
    let reply = message.chat.text("pong");
    api.send(reply).await?;
    Ok(())
}

async fn register(api: Api, message: Message, database: &Arc<Database>) -> BotResult<()> {
    let user = message.from.id;

    let res = database.insert(user)?;

    let reply = match res {
        RegisterSuccessState::Success => message.chat.text("registration success"),
        RegisterSuccessState::AlreadyRegistered => message.chat.text("already registered"),
    };

    api.send(reply).await?;

    Ok(())
}

async fn send_heartbeat(api: Api, database: Arc<Database>) -> BotResult<()> {
    let db = database
        .users
        .lock()
        .map_err(|_| anyhow!("Unable to send heartbeat: lock poisoned"))?;

    if let Some(user) = db.get(0) {
        let msg = SendMessage::new(user, "💓");
        api.send(msg).await?;
    }

    Ok(())
}

/// A database containing user ids.
/// Should be used with Arc.
struct Database {
    // We need to be able to mutate our vector, so we wrap it in a lock which can be shared across
    // threads.
    users: Mutex<Vec<UserId>>,
}

impl Database {
    /// Create a new database object in memory.
    /// Previously contained data is loaded using blocking i/o.
    ///
    /// The database should be a single text file where each line contains a single Telegram
    /// user id.
    /// If the file can't be opened, or if the read content is malformed, we return an error.
    fn open(path: impl AsRef<std::path::Path>) -> BotResult<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .map_err(|_| anyhow!("Unable to read file"))?;

        let buffer = BufReader::new(file);

        let ids = buffer
            .lines()
            .map(|line| {
                line.map_err(|_| anyhow!("Unable to access line"))
                    .and_then(|id| {
                        id.parse::<Integer>()
                            .map(|x| UserId::new(x))
                            .map_err(|_| anyhow!("Unable to parse id"))
                    })
            })
            .collect::<Result<Vec<UserId>, anyhow::Error>>()?;

        info!("Opened database");

        Ok(Self {
            users: Mutex::new(ids),
        })
    }

    /// Inserts a new user to the database.
    fn insert(&self, id: UserId) -> BotResult<RegisterSuccessState> {
        self.users
            .lock()
            .map(|mut db| {
                if !db.contains(&id) {
                    db.push(id);
                    debug!("inserted id: {}", id);

                    RegisterSuccessState::Success
                } else {
                    RegisterSuccessState::AlreadyRegistered
                }
            })
            .map_err(|_| {
                let message = "Unable to insert user id into the database";
                error!("{}", message);
                anyhow!("{}", message)
            })
    }

    // TODO
    //    fn _write_to_disk(&self) -> impl Future {
    //        Ok(())
    //    }
}

enum RegisterSuccessState {
    Success,
    AlreadyRegistered,
}
