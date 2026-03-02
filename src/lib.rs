//! # `asynclog` async logging implementation
//!
//! ## Overview
//!
//! `asynclog` is a high-performance, asynchronous logging library designed for Rust applications.
//! It offers flexible configuration options, including log level control, file output, console output,
//! asynchronous writing, and support for custom plugins and filters.
//!
//! ## Key Features
//!
//! - **Log Level Control**: Supports standard log levels (`trace`, `debug`, `info`, `warn`, `error`, `off`).
//! - **Multiple Output Targets**:
//!   - Console output (with ANSI color support)
//!   - File output (with automatic log rotation)
//! - **Asynchronous Writing**: Efficient log writing via background tasks or thread pools.
//! - **Custom Plugins**: Allows extending log output behavior.
//! - **Custom Filters**: Supports conditional filtering of log records.
//! - **Cache Optimization**: Internal caching reduces memory allocation overhead.
//! - **Cross-Platform Compatibility**: Supports both `tokio` and synchronous modes.
//!
//! ## Quick Start
//!
//! ### 1. Add Dependency
//!
//! Add the following to your [Cargo.toml](file://f:\asynclog\Cargo.toml):
//!
//! ```toml
//! [dependencies]
//! asynclog = { version = "0.1", features = ["tokio"] }
//! ```
//!
//! If asynchronous support is not needed, remove `features = ["tokio"]`.
//!
//! ### 2. Initialize Logging
//!
//! ```rust
//! use asynclog::{LogOptions, parse_level};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build log configuration
//!     let opts = LogOptions::new()
//!         .level(parse_level("info")?) // Set log level to info
//!         .log_file("app.log".to_string()) // Output to file
//!         .log_file_max_str("10M")? // Maximum log file size: 10MB
//!         .use_console(true) // Enable console output
//!         .use_async(true); // Enable asynchronous writing
//!
//!     // Initialize the logging system
//!     opts.builder()?;
//!
//!     // Start logging
//!     log::info!("Application started");
//!     log::debug!("This is a debug message");
//!     log::error!("Something went wrong!");
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Configuration Details
//!
//! ### `LogOptions` Struct
//!
//! The `LogOptions` struct provides rich configuration options for customizing logging behavior.
//!
//! #### Method List
//!
//! | Method Name        | Parameter Type              | Description                              |
//! |--------------------|-----------------------------|------------------------------------------|
//! | `level`            | `log::LevelFilter`          | Sets the minimum log level               |
//! | `log_file`         | `String`                    | Sets the log file path                   |
//! | `log_file_max`     | `u32`                       | Sets the maximum log file size (bytes)   |
//! | `use_console`      | `bool`                      | Enables/disables console output          |
//! | `use_async`        | `bool`                      | Enables/disables asynchronous writing    |
//! | `plugin`           | `Box<dyn Write + Send>`     | Registers a custom log plugin            |
//! | `filter`           | `impl CustomFilter`         | Registers a custom log filter            |
//! | `level_str`        | `&str`                      | Sets log level using a string            |
//! | `log_file_max_str` | `&str`                      | Sets max log file size using a string    |
//!
//! ## Feature Details
//!
//! ### 1. Log Levels
//!
//! Supported standard log levels include:
//!
//! - `trace`: Most detailed debugging information
//! - `debug`: Debugging information
//! - `info`: General information
//! - `warn`: Warning messages
//! - `error`: Error messages
//! - `off`: Disable all logging
//!
//! You can dynamically adjust the log level for specific modules using the `set_level` function:
//!
//! ```rust
//! asynclog::set_level("my_module".to_string(), log::LevelFilter::Warn);
//! ```
//!
//! ### 2. Asynchronous Writing
//!
//! When enabled, logs are processed by background tasks without blocking the main thread.
//! Ideal for high-concurrency scenarios.
//!
//! ```rust
//! let opts = LogOptions::new().use_async(true);
//! ```
//!
//! ### 3. Log File Rotation
//!
//! When the log file reaches its maximum size, it is automatically renamed with a `.bak` suffix,
//! and a new file is created.
//!
//! ```rust
//! let opts = LogOptions::new().log_file_max(10 * 1024 * 1024); // 10MB
//! ```
//!
//! ### 4. Custom Plugins
//!
//! Plugins can intercept each log record and execute custom logic.
//! For example, sending logs to a remote server:
//!
//! ```rust
//! use std::io::Write;
//!
//! struct RemoteLogger;
//!
//! impl Write for RemoteLogger {
//!     fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
//!         // Send log to remote server
//!         println!("Sending log: {}", String::from_utf8_lossy(buf));
//!         Ok(buf.len())
//!     }
//!
//!     fn flush(&mut self) -> std::io::Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! let opts = LogOptions::new().plugin(RemoteLogger);
//! ```
//!
//! ### 5. Custom Filters
//!
//! Implement the `CustomFilter` trait to filter logs based on arbitrary conditions:
//!
//! ```rust
//! struct KeywordFilter(String);
//!
//! impl CustomFilter for KeywordFilter {
//!     fn enabled(&self, record: &log::Record) -> bool {
//!         !record.args().to_string().contains(&self.0)
//!     }
//! }
//!
//! let opts = LogOptions::new().filter(KeywordFilter("secret".to_string()));
//! ```
//!
//! ## Notes
//!
//! - In debug mode, calling the initialization function more than once will cause the program to crash.
//! - The log file path must have write permissions.
//! - Asynchronous mode depends on the `tokio` runtime; ensure proper integration in your project.


use std::{
    io::Write,
    str::FromStr,
    sync::{OnceLock, RwLock},
};

use crossbeam::queue::ArrayQueue;
use fnv::FnvHashMap;
#[cfg(not(feature = "tokio"))]
use parking_lot::Mutex;

#[cfg(feature = "time")]
use time::format_description::OwnedFormatItem;

#[cfg(feature = "tokio")]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(not(feature = "tokio"))]
use std::{
    fs::File,
    io::{LineWriter, Stdout},
    sync::mpsc::Sender,
};

#[cfg(feature = "tokio")]
use tokio::{
    fs::File,
    io::{AsyncWriteExt, BufWriter, Stdout},
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
};

/// 格式化后的日志条目缓存最大数量, 超出该数量的Vec不进行缓存
const CACHE_STR_ARRAY_SIZE: usize = 64;
/// 缓存字符串的Vec<u8>的最大大小, 超出该大小的Vec不进行缓存
const CACHE_STR_INIT_SIZE: usize = 512;
/// 全局log对象
static ASYNC_LOGGER: OnceLock<AsyncLogger> = OnceLock::new();

/// trace_id自增值生成器
#[cfg(feature = "tokio")]
static TRACE_ID_GENERATOR: AtomicU32 = AtomicU32::new(0);

#[cfg(feature = "tokio")]
tokio::task_local! {
    /// tokio基于异步任务的共享变量(不同的任务之间有不同的trace_id)
    static TRACE_ID: u32;
}

type IoResult<T> = std::io::Result<T>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type BoxPlugin = Box<dyn std::io::Write + Send + Sync + 'static>;
type BoxCustomFilter = Box<dyn CustomFilter>;
type LevelFilter = RwLock<FnvHashMap<String, log::LevelFilter>>;
#[cfg(feature = "tokio")]
type LogRecv = UnboundedReceiver<AsyncLogType>;

#[cfg(all(feature = "time", feature = "chrono"))]
compile_error!("Only one time or chrono can be selected");

/// `LogOptions` is a struct that holds the configuration for the logger.
///
/// The `level` field is the minimum log level that will be logged. The `log_file` field is the name of
/// the log file. The `log_file_max` field is the maximum size of the log file in megabytes. The
/// `use_console` field is a boolean that indicates whether or not to log to the console. The
/// `use_async` field is a boolean that indicates whether or not to use an asynchronous logger.
///
/// The `new()` method
///
/// Properties:
///
/// * `level`: The log level to use.
/// * `log_file`: The name of the log file.
/// * `log_file_max`: The maximum number of log files to keep.
/// * `use_console`: If true, the logger will log to the console.
/// * `use_async`: Whether to use the async logger or not.
/// * `filter`: Use user-defined log filtering functions.
/// * `plugin`: Use a user-defined log output plug-in.
///
/// # Examples
///
/// ```rust
/// asnyclog::Builder::new()
///     .level(log::LevelFilter::Debug)
///     .log_file(String::from("./app.log"))
///     .log_file_max(1024 * 1024)
///     .use_console(true)
///     .use_async(true)
///     .filter(|r: &log::Record| r.target() != "gensql::sql_builder" || r.line().unwrap_or(0) != 72)
///     .builder()?;
/// ```
pub struct LogOptions {
    /// log level
    level: log::LevelFilter,
    /// The log file path. ignore if the value is empty
    log_file: String,
    /// The maximum size of the log file, The units that can be used are k/m/g.
    log_file_max: u32,
    /// Whether to output to the console
    use_console: bool,
    /// Whether to use asynchronous logging, if true, the log will be written to the file in
    /// a separate thread, and the log will not be blocked.
    use_async: bool,
    /// The log plugin, custom function for every log record
    plugin: Option<BoxPlugin>,
    /// The log filter, ignore log if record.target exists in the filter
    filter: Option<BoxCustomFilter>,
}

/// custom log filter
pub trait CustomFilter: Send + Sync + 'static {
    fn enabled(&self, record: &log::Record) -> bool;
}

struct AsyncLogger {
    /// 日志的有效级别，小于该级别的日志允许输出
    level: log::LevelFilter,
    /// 日志条目时间格式化样式
    #[cfg(feature = "time")]
    dt_fmt: OwnedFormatItem,
    /// 日志条目时间格式化样式
    #[cfg(not(feature = "time"))]
    dt_fmt: String,
    /// 日志文件名
    log_file: String,
    /// 日志文件允许的最大长度
    max_size: u32,
    /// 用户自定义的过滤目标->过滤级别映射
    level_filter: LevelFilter,
    /// 格式化日志条目时，从该处获取缓存变量存放最后格式化结果
    fmt_cache: ArrayQueue<Vec<u8>>,
    /// 异步模式下，用于暂存等待写入日志文件的日志条目
    #[cfg(feature = "tokio")]
    msg_tx: UnboundedSender<AsyncLogType>,
    /// 自定义日志过滤器，如果启用了自定义日志过滤器，则对象有值
    filter: Option<BoxCustomFilter>,
    /// 日志关联的动态变化的数据
    #[cfg(not(feature = "tokio"))]
    logger_data: Mutex<LogData>,
}

struct LogData {
    /// 当前日志文件的大小，跟随写入新的日志内容而变化
    log_size: u32,
    #[cfg(feature = "tokio")]
    /// 是否输出到文件
    use_file: bool,
    /// 异步控制台对象，如果启用了控制台输出，则对象有值
    #[cfg(feature = "tokio")]
    console: Option<BufWriter<Stdout>>,
    /// 控制台对象，如果启用了控制台输出，则对象有值
    #[cfg(not(feature = "tokio"))]
    console: Option<LineWriter<Stdout>>,
    /// 异步文件对象，如果启用了文件输出，则对象有值
    #[cfg(feature = "tokio")]
    fileout: Option<BufWriter<File>>,
    /// 文件对象，如果启用了文件输出，则对象有值
    #[cfg(not(feature = "tokio"))]
    fileout: Option<LineWriter<File>>,
    /// 异步发送频道，如果启用了异步日志模式，则对象有值
    #[cfg(not(feature = "tokio"))]
    sender: Option<Sender<AsyncLogType>>,
    /// 插件对象，如果启用了插件输出，则对象有值
    plugin: Option<BoxPlugin>,
}

/// 基于日志文本内容的去除ansi颜色信息的迭代器, 每次迭代获取不包含ansi色彩设置的文本内容
struct SkipAnsiColorIter<'a> {
    data: &'a [u8],
    pos: usize,
    find_len: usize,
}

/// 异步模式中的消息类型
enum AsyncLogType {
    Message(Vec<u8>),
    Flush,
}

/// get a new trace_id value of next from current trace_id
#[cfg(feature = "tokio")]
pub fn next_trace_id() -> u32 {
    TRACE_ID_GENERATOR.fetch_add(1, Ordering::Relaxed)
}

/// get current trace_id value
#[cfg(feature = "tokio")]
pub fn get_trace_id() -> Option<u32> {
    TRACE_ID.try_get().ok()
}

/// Set log level for target
///
/// Arguments:
///
/// * `target`: log target
/// * `level`: The log level(off/error/warn/info/debug/trace)
pub fn set_level(target: String, level: log::LevelFilter) {
    if let Ok(mut f) = get_async_logger().level_filter.write() {
        f.insert(target, level);
    }
}

/// It takes a string and returns a `Result` of a `log::LevelFilter`
///
/// Arguments:
///
/// * `level`: The log level(off/error/warn/info/debug/trace) to parse.
///
/// Returns:
///
/// A Result<log::LevelFilter>
pub fn parse_level(level: &str) -> Result<log::LevelFilter> {
    match log::LevelFilter::from_str(level) {
        Ok(num) => Ok(num),
        Err(_) => Err(format!("can't parse log level: {level}").into()),
    }
}

/// It parses a string into a number, The units that can be used are k/m/g
///
/// Arguments:
///
/// * `size`: The size of the file to be generated(uints: k/m/g).
///
/// Returns:
///
/// A Result<u32, anyhow::Error>
pub fn parse_size(size: &str) -> Result<u32> {
    match size.parse() {
        Ok(n) => Ok(n),
        Err(_) => match size[..size.len() - 1].parse() {
            Ok(n) => {
                let s = size.as_bytes();
                match s[s.len() - 1] {
                    b'b' | b'B' => Ok(n),
                    b'k' | b'K' => Ok(n * 1024),
                    b'm' | b'M' => Ok(n * 1024 * 1024),
                    b'g' | b'G' => Ok(n * 1024 * 1024 * 1024),
                    _ => Err(format!("parse size error, unit is unknown: {size}").into()),
                }
            }
            Err(e) => Err(e.into()),
        },
    }
}

/// It creates a new logger, initializes it, and then sets it as the global logger
pub fn init_log(opts: LogOptions) -> Result<()> {
    #[cfg(debug_assertions)]
    debug_check_init();

    log::set_max_level(opts.level);

    #[cfg(feature = "time")]
    let dt_fmt = {
        let fmt = "[month]-[day] [hour]:[minute]:[second]";
        time::format_description::parse_owned::<2>(fmt).unwrap()
    };
    #[cfg(feature = "chrono")]
    let dt_fmt = "%m-%d %H:%M:%S".to_string();

    // 如果启用控制台输出，创建一个控制台共享句柄
    #[cfg(not(feature = "tokio"))]
    let console = if opts.use_console {
        Some(LineWriter::new(std::io::stdout()))
    } else {
        None
    };
    #[cfg(feature = "tokio")]
    let console = if opts.use_console {
        Some(BufWriter::new(tokio::io::stdout()))
    } else {
        None
    };

    #[cfg(not(feature = "tokio"))]
    {
        let lf = &opts.log_file;
        // 如果启用文件输出，打开日志文件
        let (fileout, log_size) = if !lf.is_empty() {
            let f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(lf)?;
            let log_size = std::fs::metadata(lf)?.len() as u32;
            let fileout = Some(LineWriter::new(f));
            (fileout, log_size)
        } else {
            (None, 0)
        };

        // 如果启用异步日志，开启一个线程不停读取channel中的数据进行日志写入，属于多生产者单消费者模式
        let sender = if opts.use_async {
            let (sender, receiver) = std::sync::mpsc::channel::<AsyncLogType>();
            std::thread::spawn(move || {
                loop {
                    match receiver.recv() {
                        Ok(data) => match data {
                            AsyncLogType::Message(msg) => {
                                get_async_logger().write(&msg);
                                put_msg_to_cache(msg);
                            }
                            AsyncLogType::Flush => get_async_logger().flush_inner(),
                        },
                        Err(e) => eprintln!("logger channel recv error: {}", e),
                    }
                }
            });
            Some(sender)
        } else {
            None
        };

        let logger = AsyncLogger {
            level: opts.level,
            dt_fmt,
            log_file: opts.log_file,
            max_size: opts.log_file_max,
            level_filter: RwLock::new(FnvHashMap::default()),
            fmt_cache: ArrayQueue::new(CACHE_STR_ARRAY_SIZE),
            filter: opts.filter,
            logger_data: Mutex::new(LogData {
                log_size,
                console,
                fileout,
                sender,
                plugin: opts.plugin,
            }),
        };
        if ASYNC_LOGGER.set(logger).is_err() {
            panic!("async logger init failed");
        }

        // 设置全局日志对象
        log::set_logger(get_async_logger()).expect("init_log call set_logger error");
    }

    #[cfg(feature = "tokio")]
    {
        let use_file = !opts.log_file.is_empty();
        let (tx, rx) = unbounded_channel();

        let logger = AsyncLogger {
            level: opts.level,
            dt_fmt,
            log_file: opts.log_file,
            max_size: opts.log_file_max,
            level_filter: RwLock::new(FnvHashMap::default()),
            fmt_cache: ArrayQueue::new(CACHE_STR_ARRAY_SIZE),
            msg_tx: tx,
            filter: opts.filter,
        };
        if ASYNC_LOGGER.set(logger).is_err() {
            panic!("async logger init failed");
        }

        // 设置全局日志对象
        log::set_logger(get_async_logger()).expect("init_log call set_logger error");

        tokio::spawn(write_async(
            LogData {
                log_size: 0,
                use_file,
                console,
                fileout: None,
                plugin: opts.plugin,
            },
            rx,
        ));
    }

    Ok(())
}

#[cfg(not(feature = "tokio"))]
impl AsyncLogger {
    /// 输出日志到控制台和文件
    fn write(&self, msg: &[u8]) {
        if msg.is_empty() {
            return;
        }

        let mut logger_data = self.logger_data.lock();

        // 如果启用了控制台输出，则写入控制台
        if let Some(ref mut console) = logger_data.console {
            let _ = console.write_all(msg);
        }

        // 判断日志长度是否到达最大限制，如果到了，需要备份当前日志文件并重新创建新的日志文件
        if logger_data.log_size > self.max_size {
            // 如果启用了日志文件，刷新缓存并关闭日志文件
            let has_file = match logger_data.fileout {
                Some(ref mut fileout) => {
                    let _ = fileout.flush();
                    logger_data.fileout.take();
                    true
                }
                None => false,
            };

            // 之所以把关闭文件和重新创建文件分开写，是因为rust限制了可变借用(fileout)只允许1次
            if has_file {
                // 删除已有备份，并重命名现有文件为备份文件
                let bak = format!("{}.bak", self.log_file);
                if std::fs::remove_file(&bak).is_err() {
                    eprint!("remove file error: {bak}");
                }
                if std::fs::rename(&self.log_file, &bak).is_err() {
                    eprint!("backup log file fail: {} -> {bak}", &self.log_file);
                }

                match std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&self.log_file)
                {
                    Ok(file) => {
                        logger_data.fileout = Some(LineWriter::new(file));
                        logger_data.log_size = 0;
                    }
                    Err(_) => {
                        eprint!("open log file error: {}", self.log_file);
                    }
                }
            }
        }

        if let Some(ref mut fileout) = logger_data.fileout {
            let ws = write_text(fileout, msg).unwrap_or(0);
            logger_data.log_size += ws as u32;
        }

        if let Some(plugin) = &mut logger_data.plugin
            && let Err(e) = plugin.write_all(msg)
        {
            eprint!("plugin write error: {e:?}");
        }
    }

    /// 刷新日志的控制台和文件缓存
    fn flush_inner(&self) {
        let mut logger_data = self.logger_data.lock();

        if let Some(ref mut console) = logger_data.console {
            console.flush().expect("flush log console error");
        }

        if let Some(ref mut fileout) = logger_data.fileout {
            fileout.flush().expect("flush log file error");
        }
    }
}

impl AsyncLogger {
    /// 格式化日志记录
    fn fmt_record(&self, record: &log::Record) -> Option<Vec<u8>> {
        use log::Log;

        // 判断当前条目是否被允许输出
        if !self.enabled(record.metadata()) {
            return None;
        }
        if let Some(filter) = &self.filter
            && !filter.enabled(record)
        {
            return None;
        }

        let now = now_as_str(&self.dt_fmt);
        let mut msg = get_msg_from_cache();
        let is_detail = self.level >= log::LevelFilter::Debug;
        let log_level = record.level();

        // 格式化时间及日志级别
        if is_detail {
            let lc = level_color(log_level);
            let _ = write!(
                &mut msg,
                "[\x1b[36m{now}\x1b[0m] [{lc}{log_level:5}\x1b[0m]"
            );
        } else {
            let _ = write!(&mut msg, "[{now}] [{log_level:5}]");
        }

        // 格式化请求追踪标志(优先级别: task_id > trace_id > thread_name)
        #[cfg(feature = "tokio")]
        if let Some(task_id) = tokio::task::try_id() {
            let _ = write!(&mut msg, " [\x1b[34mTASK:{task_id}\x1b[0m]");
        } else if let Some(trace_id) = get_trace_id() {
            let _ = write!(&mut msg, " [\x1b[35mTRACE:{trace_id}\x1b[0m]");
        } else {
            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("unknown");
            let _ = write!(&mut msg, " [\x1b[32mTHREAD:{thread_name}\x1b[0m]");
        }
        #[cfg(not(feature = "tokio"))]
        {
            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("unknown");
            let _ = write!(&mut msg, " [\x1b[32mTHREAD:{thread_name}\x1b[0m]");
        }

        // 格式化对应的库名称及行号
        if is_detail {
            let target = record.target();
            let line = record.line().unwrap_or(0);
            let _ = write!(&mut msg, " [{target}::{line}]");
        };

        // 格式化日志内容
        #[allow(clippy::write_with_newline)]
        let _ = write!(&mut msg, " - {}\n", record.args());

        Some(msg)
    }
}


impl log::Log for AsyncLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if metadata.level() <= self.level
            && let Ok(level_filters) = self.level_filter.read()
        {
            let mut target = metadata.target();
            while !target.is_empty() {
                if let Some(level) = level_filters.get(target) {
                    return metadata.level() <= *level;
                }

                target = match target.rfind("::") {
                    Some(rpos) => &target[..rpos],
                    None => "",
                };
            }
            return true;
        }
        false
    }

    #[cfg(feature = "tokio")]
    fn log(&self, record: &log::Record) {
        let msg = match self.fmt_record(record) {
            Some(msg) => AsyncLogType::Message(msg),
            None => return,
        };

        // 将需要输出的日志条目加入队列尾部，并返回是否已有任务正在处理日志
        if let Err(e) = get_async_logger().msg_tx.send(msg) {
            eprintln!("failed in log::Log.log, {e:?}");
        }
    }

    #[cfg(not(feature = "tokio"))]
    fn log(&self, record: &log::Record) {
        let msg = match self.fmt_record(record) {
            Some(msg) => msg,
            None => return,
        };

        // 异步写入模式
        let logger_data = self.logger_data.lock();
        if let Some(ref sender) = logger_data.sender {
            // 采用独立的单线程写入日志的方式，向channel发送要写入的日志消息即可
            let _ = sender.send(AsyncLogType::Message(msg));
            return;
        }

        // 同步写入模式
        self.write(&msg);
        put_msg_to_cache(msg);
    }

    #[cfg(feature = "tokio")]
    fn flush(&self) {
        tokio::spawn(async move {
            if let Err(e) = get_async_logger().msg_tx.send(AsyncLogType::Flush) {
                eprintln!("failed in log send to channel: {e:?}");
            }
        });
    }

    #[cfg(not(feature = "tokio"))]
    fn flush(&self) {
        let logger_data = self.logger_data.lock();
        if let Some(ref sender) = logger_data.sender {
            if let Err(e) = sender.send(AsyncLogType::Flush) {
                eprint!("failed in log::flush: {e:?}");
            }
        } else {
            drop(logger_data);
            self.flush_inner();
        }
    }
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            level: log::LevelFilter::Info,
            log_file: String::new(),
            log_file_max: 10 * 1024 * 1024,
            use_console: true,
            use_async: true,
            plugin: None,
            filter: None,
        }
    }
}

impl LogOptions {
    /// create a new log options with default options
    pub fn new() -> Self {
        Self::default()
    }

    /// create a new log with options
    pub fn builder(self) -> Result<()> {
        init_log(self)
    }

    /// set log level, default: log::LevelFilter::Info
    pub fn level(mut self, level: log::LevelFilter) -> Self {
        self.level = level;
        self
    }

    /// Set the name of the log file, which is empty by default
    pub fn log_file(mut self, log_file: String) -> Self {
        self.log_file = log_file;
        self
    }

    /// Configure the maximum log file size, the default is 10M
    pub fn log_file_max(mut self, log_file_max: u32) -> Self {
        self.log_file_max = log_file_max;
        self
    }

    /// Whether to use console output. Defaults to true
    pub fn use_console(mut self, use_console: bool) -> Self {
        self.use_console = use_console;
        self
    }

    /// Whether to use asynchronous output. Defaults to true
    pub fn use_async(mut self, use_async: bool) -> Self {
        self.use_async = use_async;
        self
    }

    /// Setting up plugins
    pub fn plugin<T: std::io::Write + Send + Sync + 'static>(mut self, plugin: T) -> Self {
        self.plugin = Some(Box::new(plugin));
        self
    }

    /// Set up custom filters
    pub fn filter(mut self, filter: impl CustomFilter) -> Self {
        self.filter = Some(Box::new(filter));
        self
    }

    /// Set the logging level (string, support the "trace", "debug", "info", "warn", "error", "off")
    pub fn level_str(mut self, level: &str) -> Result<Self> {
        self.level = parse_level(level)?;
        Ok(self)
    }

    /// Set the log file size (string, support the "K", "M", "G", for example: "10 M" or "G")
    pub fn log_file_max_str(mut self, log_file_max: &str) -> Result<Self> {
        self.log_file_max = parse_size(log_file_max)?;
        Ok(self)
    }
}

impl<F: Fn(&log::Record) -> bool + Send + Sync + 'static> CustomFilter for F {
    fn enabled(&self, record: &log::Record) -> bool {
        self(record)
    }
}

impl<'a> SkipAnsiColorIter<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let find_len = if data.len() > 3 { data.len() - 3 } else { 0 };

        SkipAnsiColorIter {
            data,
            pos: 0,
            find_len,
        }
    }
}

impl<'a> Iterator for SkipAnsiColorIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        // 过滤ansi颜色
        let (mut pos, find_len, data) = (self.pos, self.find_len, self.data);
        while pos < find_len {
            unsafe {
                if *data.get_unchecked(pos) != 0x1b || *data.get_unchecked(pos + 1) != b'[' {
                    pos += 1;
                    continue;
                }

                // 找到ansi颜色前缀，返回前缀前的字符串并更新当前位置和已写入字节
                let n = if *data.get_unchecked(pos + 3) == b'm' {
                    4
                } else {
                    5
                };
                let p = self.pos;
                self.pos = pos + n;
                return Some(&data[p..pos]);
            }
        }

        // 写入剩余的数据
        let dl = data.len();
        if pos < dl {
            let p = self.pos;
            self.pos = dl;
            return Some(&data[p..dl]);
        }

        None
    }
}

/// 将格式化后的消息内容同步写入日志文件
#[cfg(not(feature = "tokio"))]
fn write_text(w: &mut LineWriter<File>, msg: &[u8]) -> IoResult<usize> {
    let mut write_len = 0;
    for item in SkipAnsiColorIter::new(msg) {
        write_len += item.len();
        w.write_all(item)?;
    }

    // 如果已换行符结尾, 则刷新缓冲区
    let len = msg.len();
    if len > 0 && (msg[len - 1] == b'\n' || msg[len - 1] == b'\r') {
        w.flush()?;
    }

    Ok(write_len)
}

/// 将格式化后的消息内容异步写入日志文件
#[cfg(feature = "tokio")]
async fn write_text(w: &mut BufWriter<File>, msg: &[u8]) -> IoResult<usize> {
    let mut write_len = 0;
    for item in SkipAnsiColorIter::new(msg) {
        write_len += item.len();
        w.write_all(item).await?;
    }

    // 如果已换行符结尾, 则刷新缓冲区
    let len = msg.len();
    if len > 0 && (msg[len - 1] == b'\n' || msg[len - 1] == b'\r') {
        w.flush().await?;
    }

    Ok(write_len)
}

/// 异步日志响应任务, 从通道中获取日志消息并并进行相应的操作
#[cfg(feature = "tokio")]
async fn write_async(mut log_data: LogData, mut rx: LogRecv) {
    // 首次使用，初始化日志文件
    if log_data.use_file && log_data.fileout.is_none() {
        let alog = get_async_logger();
        let (fileout, size) = open_log_file(&alog.log_file, true).await;
        log_data.fileout = fileout;
        log_data.log_size = size;
    }

    // 循环读取消息队列并输出，当消息队列为空时，设置任务终止标志并终止任务
    while let Some(data) = rx.recv().await {
        match data {
            AsyncLogType::Message(msg) => {
                // 写入日志消息
                write_to_log(&mut log_data, &msg).await;
                // 回收msg到缓存队列，以便下次使用
                put_msg_to_cache(msg);
            }
            AsyncLogType::Flush => {
                if let Some(ref mut console) = log_data.console
                    && let Err(e) = console.flush().await
                {
                    println!("flush log to console failed: {e:?}");
                }
                if let Some(ref mut fileout) = log_data.fileout
                    && let Err(e) = fileout.flush().await
                {
                    println!("flush log to file failed: {e:?}");
                }
            }
        }
    }
}

/// 打开日志文件
#[cfg(feature = "tokio")]
async fn open_log_file(log_file: &str, append: bool) -> (Option<BufWriter<File>>, u32) {
    let file_ret = tokio::fs::OpenOptions::new()
        .append(append)
        .write(true)
        .create(true)
        .open(log_file)
        .await;

    match file_ret {
        Ok(file) => {
            let size = if append {
                let fmeta = tokio::fs::metadata(log_file).await;
                fmeta.map_or_else(|_| 0, |m| m.len())
            } else {
                0
            };
            (Some(BufWriter::new(file)), size as u32)
        }
        Err(e) => {
            eprintln!("open log file failed: {e:?}");
            (None, 0)
        }
    }
}

/// 原日志文件备份为bak后缀, 重新新建日志文件
#[cfg(feature = "tokio")]
async fn rolling_file(log_data: &mut LogData) {
    // 刷新日志缓存
    let has_file = match log_data.fileout {
        Some(ref mut fileout) => {
            // 刷新缓存并关闭日志文件
            if let Err(e) = fileout.flush().await {
                eprintln!("failed in log flush log file: {e:?}");
            }
            true
        }
        None => false,
    };

    if !has_file {
        return;
    };

    log_data.fileout.take();

    // 之所以把关闭文件和重新创建文件分开写，是因为rust限制了可变借用(fileout)只允许1次
    // 删除已有备份，并重命名现有文件为备份文件
    let alog = get_async_logger();
    let bak = format!("{}.bak", alog.log_file);
    let _ = tokio::fs::remove_file(&bak).await;
    match tokio::fs::rename(&alog.log_file, &bak).await {
        Ok(_) => {
            let (file, _) = open_log_file(&alog.log_file, false).await;
            log_data.fileout = file;
            log_data.log_size = 0;
        }
        Err(e) => eprintln!("failed in log rename file: {e:?}"),
    }
}

#[cfg(feature = "tokio")]
async fn write_to_log(log_data: &mut LogData, msg: &[u8]) {
    if msg.is_empty() {
        return;
    }

    // 如果启用了控制台输出，则写入控制台
    if let Some(ref mut console) = log_data.console
        && console.write_all(msg).await.is_ok()
    {
        let c = msg[msg.len() - 1];
        if c == b'\n' || c == b'\r' {
            let _ = console.flush().await;
        }
    }

    // 判断日志长度是否到达最大限制，如果到了，需要备份当前日志文件并重新创建新的日志文件
    if log_data.log_size > get_async_logger().max_size {
        rolling_file(log_data).await
    }

    if let Some(ref mut fileout) = log_data.fileout {
        match write_text(fileout, msg).await {
            Ok(size) => log_data.log_size += size as u32,
            Err(e) => eprintln!("failed in write to file: {e:?}"),
        }
    }

    // 日志输出到插件中(如果存在的话)
    if let Some(ref mut plugin) = log_data.plugin
        && let Err(e) = plugin.write_all(msg)
    {
        eprintln!("failed in log plugin: {e:?}");
    }
}

fn get_async_logger() -> &'static AsyncLogger {
    match ASYNC_LOGGER.get() {
        Some(logger) => logger,
        None => unsafe { std::hint::unreachable_unchecked() },
    }
}

/// 返回日志级别对应的ansi颜色
fn level_color(level: log::Level) -> &'static str {
    // const RESET:    &str = "\x1b[0m";
    // const BLACK:    &str = "\x1b[30m";
    const RED: &str = "\x1b[31m";
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const BLUE: &str = "\x1b[34m";
    const MAGENTA: &str = "\x1b[35m";
    // const CYAN:     &str = "\x1b[36m";
    // const WHITE:    &str = "\x1b[37m";

    match level {
        log::Level::Trace => GREEN,
        log::Level::Debug => YELLOW,
        log::Level::Info => BLUE,
        log::Level::Warn => MAGENTA,
        log::Level::Error => RED,
    }
}

/// 获取当前时间并格式化为字符串返回
#[cfg(feature = "time")]
fn now_as_str(fmt: &OwnedFormatItem) -> String {
    if let Ok(t) = time::OffsetDateTime::now_local()
        && let Ok(s) = t.format(fmt)
    {
        s
    } else {
        String::new()
    }
}

#[cfg(not(feature = "time"))]
fn now_as_str(fmt: &str) -> String {
    chrono::Local::now().format(fmt)
}

/// 获取一个用于保存格式化日志条目的对象, 优先从缓存获取，缓存没有则新建一个
fn get_msg_from_cache() -> Vec<u8> {
    get_async_logger().fmt_cache.pop()
        .unwrap_or_else(|| Vec::with_capacity(CACHE_STR_INIT_SIZE))
}

/// 回收Vec<u8>对象，用于下次使用，避免频繁分配内存造成内存碎片
fn put_msg_to_cache(mut value: Vec<u8>) {
    if value.capacity() <= CACHE_STR_INIT_SIZE {
        value.clear();
        let _ = get_async_logger().fmt_cache.push(value);
    }
}

/// 调试模式下, 只能执行一次的函数, 用于校验初始化日志函数是否被调用1次以上, 如果是, 则异常结束程序
#[cfg(debug_assertions)]
fn debug_check_init() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INITED: AtomicBool = AtomicBool::new(false);

    let r = INITED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
    if r.is_err() {
        panic!("init_log must run once!");
    }
}
