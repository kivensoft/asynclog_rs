use std::{collections::HashMap, io::Write, str::FromStr, sync::OnceLock};

use crossbeam::queue::ArrayQueue;
use parking_lot::{RwLock, Mutex};

#[cfg(feature = "time")]
use time::format_description::OwnedFormatItem;

#[cfg(not(feature = "tokio"))]
use std::{fs::File, io::{LineWriter, Stdout}, sync::mpsc::Sender};

#[cfg(feature = "tokio")]
use tokio::{fs::File, io::{AsyncWriteExt, BufWriter, Stdout}};
#[cfg(feature = "tokio")]
use std::collections::VecDeque;

const CACHE_STR_ARRAY_SIZE: usize = 16;
const CACHE_STR_INIT_SIZE: usize = 256;

static LEVEL_FILTERS: OnceLock<RwLock<HashMap<String, log::LevelFilter>>> = OnceLock::new();
static CACHE_STRS: OnceLock<ArrayQueue<Vec<u8>>> = OnceLock::new();
#[cfg(feature = "tokio")]
static mut ASYNC_LOGGER: Option<&AsyncLogger> = None;
#[cfg(feature = "tokio")]
static MSG_QUEUE: Mutex<(VecDeque<Vec<u8>>, bool)> = Mutex::new((VecDeque::new(), false));

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type BoxPlugin = Box<dyn std::io::Write + Send + 'static>;

/// `Builder` is a struct that holds the configuration for the logger.
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
///
/// # Examples
///
/// ```
/// asnyclog::Builder::new()
///     .level(log::LevelFilter::Debug)
///     .log_file(String::from("./app.log"))
///     .log_file_max(1024 * 1024)
///     .use_console(true)
///     .use_async(true)
///     .builder()?;
/// ```
pub struct Builder {
    level:          log::LevelFilter,
    log_file:       String,
    log_file_max:   u32,
    use_console:    bool,
    use_async:      bool,
    plugin:         Option<BoxPlugin>,
}

struct AsyncLogger {
    level:          log::LevelFilter,               // 日志的有效级别，小于该级别的日志允许输出
    #[cfg(feature = "time")]
    dt_fmt:         OwnedFormatItem,                // 日志条目时间格式化样式
    #[cfg(feature = "chrono")]
    dt_fmt:         String,                         // 日志条目时间格式化样式
    log_file:       String,                         // 日志文件名
    max_size:       u32,                            // 日志文件允许的最大长度
    #[cfg(not(feature = "tokio"))]
    logger_data:    Mutex<LogData>,                 // 日志关联的动态变化的数据
    #[cfg(feature = "tokio")]
    logger_data:    tokio::sync::Mutex<LogData>,    // 日志关联的动态变化的数据
}

struct LogData {
    log_size:       u32,                            // 当前日志文件的大小，跟随写入新的日志内容而变化
    #[cfg(feature = "tokio")]
    use_console:    bool,                           // 是否输出到控制台
    #[cfg(feature = "tokio")]
    use_file:       bool,                           // 是否输出到文件
    #[cfg(feature = "tokio")]
    console:        Option<BufWriter<Stdout>>,      // 控制台对象，如果启用了控制台输出，则对象有值
    #[cfg(not(feature = "tokio"))]
    console:        Option<LineWriter<Stdout>>,     // 控制台对象，如果启用了控制台输出，则对象有值
    #[cfg(feature = "tokio")]
    fileout:        Option<BufWriter<File>>,        // 文件对象，如果启用了文件输出，则对象有值
    #[cfg(not(feature = "tokio"))]
    fileout:        Option<LineWriter<File>>,       // 文件对象，如果启用了文件输出，则对象有值
    #[cfg(not(feature = "tokio"))]
    sender:         Option<Sender<AsyncLogType>>,   // 异步发送频道，如果启用了异步日志模式，则对象有值
    plugin:         Option<BoxPlugin>,              // 插件对象，如果启用了插件输出，则对象有值
}

struct SkipAnsiColorIter<'a> {
    data: &'a [u8],
    pos: usize,
    find_len: usize,
}

#[cfg(not(feature = "tokio"))]
enum AsyncLogType {
    Message(Vec<u8>),
    Flush,
}


#[inline]
pub fn init_log_simple(level: &str, log_file: String, log_file_max: &str,
        use_console: bool, use_async: bool) -> Result<()> {
    init_log(parse_level(level)?, log_file, parse_size(log_file_max)?, use_console, use_async)
}

/// It creates a new logger, initializes it, and then sets it as the global logger
///
/// Arguments:
///
/// * `level`: log level
/// * `log_file`: The log file path. ignore if the value is empty
/// * `log_file_max`: The maximum size of the log file, The units that can be used are k/m/g.
/// * `use_console`: Whether to output to the console
/// * `use_async`: Whether to use asynchronous logging, if true, the log will be written to the file in
/// a separate thread, and the log will not be blocked.
///
/// Returns:
///
/// A Result<(), anyhow::error>
///
/// # Examples
///
/// ```
/// asnyclog::init_log(log::LevelFilter::Debug, String::from("./app.log", 1024 * 1024, true, true)?;
/// ````
#[inline]
pub fn init_log(level: log::LevelFilter, log_file: String, log_file_max: u32,
        use_console: bool, use_async: bool) -> Result<()> {
    init_log_with_plugin(level, log_file, log_file_max, use_console, use_async, None)
}

#[cfg(feature = "tokio")]
#[inline]
pub fn init_log_with_plugin(level: log::LevelFilter, log_file: String, log_file_max: u32,
        use_console: bool, use_async: bool, plugin: Option<Box<dyn std::io::Write + Send>>) -> Result<()> {

    #[cfg(debug_assertions)]
    debug_check_init();

    let _ = use_async;
    log::set_max_level(level);

    #[cfg(feature = "time")]
    let dt_fmt = if level >= log::LevelFilter::Debug {
        "[month]-[day] [hour]:[minute]:[second]"
    } else {
        "[year]-[month]-[day] [hour]:[minute]:[second]"
    };
    #[cfg(feature = "time")]
    let dt_fmt = time::format_description::parse_owned::<2>(dt_fmt).unwrap();

    #[cfg(feature = "chrono")]
    let dt_fmt = if level >= log::LevelFilter::Debug {
        "%m-%d %H:%M:%S"
    } else {
        "%Y-%m-%d %H:%M:%S"
    }.to_owned();

    let use_file = !log_file.is_empty();
    let logger = Box::new(AsyncLogger {
        level,
        dt_fmt,
        log_file,
        max_size: log_file_max,
        logger_data: tokio::sync::Mutex::new(LogData {
            log_size: 0,
            use_console,
            use_file,
            console: None,
            fileout: None,
            plugin,
        }),
    });

    let logger = Box::leak(logger);
    unsafe {
        ASYNC_LOGGER = Some(logger);
    }

    // 设置全局日志对象
    log::set_logger(logger).expect("init_log call set_logger error");

    Ok(())
}

#[cfg(not(feature = "tokio"))]
#[inline]
pub fn init_log_with_plugin(level: log::LevelFilter, log_file: String, log_file_max: u32,
        use_console: bool, use_async: bool, plugin: Option<Box<dyn Write + Send>>) -> Result<()> {

    #[cfg(debug_assertions)]
    debug_check_init();

    log::set_max_level(level);

    #[cfg(feature = "time")]
    let dt_fmt = if level >= log::LevelFilter::Debug {
        "[month]-[day] [hour]:[minute]:[second]"
    } else {
        "[year]-[month]-[day] [hour]:[minute]:[second]"
    };
    #[cfg(feature = "time")]
    let dt_fmt = time::format_description::parse_owned::<2>(dt_fmt).unwrap();

    #[cfg(feature = "chrono")]
    let dt_fmt = if level >= log::LevelFilter::Debug {
        "%m-%d %H:%M:%S"
    } else {
        "%Y-%m-%d %H:%M:%S"
    }.to_owned();


    let logger = Box::new(AsyncLogger {
        level,
        dt_fmt,
        log_file,
        max_size: log_file_max,
        logger_data: Mutex::new(LogData {
            log_size: 0,
            console: None,
            fileout: None,
            sender: None,
            plugin,
        }),
    });

    let logger = Box::leak(logger);
    let mut logger_data = logger.logger_data.lock();

    // 如果启用控制台输出，创建一个控制台共享句柄
    if use_console {
        logger_data.console = Some(LineWriter::new(std::io::stdout()));
    }

    // 如果启用文件输出，打开日志文件
    if !logger.log_file.is_empty() {
        let f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&logger.log_file)?;
        logger_data.log_size = std::fs::metadata(&logger.log_file)?.len() as u32;
        logger_data.fileout = Some(LineWriter::new(f));
    }

    // 如果启用异步日志，开启一个线程不停读取channel中的数据进行日志写入，属于多生产者单消费者模式
    if use_async {
        let (sender, receiver) = std::sync::mpsc::channel::<AsyncLogType>();
        logger_data.sender = Some(sender);
        let logger: &AsyncLogger = logger;
        std::thread::spawn(move || loop {
            match receiver.recv() {
                Ok(data) => match data {
                    AsyncLogType::Message(mut msg) => {
                        logger.write(&msg);
                        msg.truncate(0);
                        let _ = get_cache_strs().push(msg);
                    },
                    AsyncLogType::Flush => logger.flush_inner(),
                },
                Err(e) => {
                    panic!("logger channel recv error: {}", e);
                }
            }
        });
    }

    // 设置全局日志对象
    log::set_logger(logger).expect("init_log call set_logger error");

    Ok(())
}

/// Set log level for target
///
/// Arguments:
///
/// * `target`: log target
/// * `level`: The log level(off/error/warn/info/debug/trace)
pub fn set_level(target: String, level: log::LevelFilter) {
    let mut level_filters = get_level_filters().write();
    level_filters.insert(target, level);
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
            },
            Err(e) => Err(e.into()),
        }
    }
}


#[cfg(not(feature = "tokio"))]
impl AsyncLogger {
    // 输出日志到控制台和文件
    fn write(&self, msg: &[u8]) {
        let mut logger_data = self.logger_data.lock();

        // 如果启用了控制台输出，则写入控制台
        if let Some(ref mut console) = logger_data.console {
            console.write_all(msg).expect("write log to console fail");
        }

        // 判断日志长度是否到达最大限制，如果到了，需要备份当前日志文件并重新创建新的日志文件
        if logger_data.log_size > self.max_size {
            let mut log_file_closed = false;

            // 如果启用了日志文件，刷新缓存并关闭日志文件
            if let Some(ref mut fileout) = logger_data.fileout {
                fileout.flush().expect("flush log file fail");
                logger_data.fileout.take();
                log_file_closed = true;
            }

            // 之所以把关闭文件和重新创建文件分开写，是因为rust限制了可变借用(fileout)只允许1次
            if log_file_closed {
                // 删除已有备份，并重命名现有文件为备份文件
                let bak = format!("{}.bak", self.log_file);
                std::fs::remove_file(&bak).unwrap_or_default();
                std::fs::rename(&self.log_file, &bak).expect("backup log file fail");

                let f = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .open(&self.log_file)
                        .expect("reopen log file fail");

                logger_data.fileout = Some(LineWriter::new(f));
                logger_data.log_size = 0;
            }
        }

        if let Some(ref mut fileout) = logger_data.fileout {
            let ws = write_text(fileout, msg).unwrap();
            logger_data.log_size += ws as u32;
        }

        if let Some(ref mut plugin) = logger_data.plugin {
            plugin.write_all(msg).expect("write log to plugin fail");
        }
    }

    // 刷新日志的控制台和文件缓存
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


impl log::Log for AsyncLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if metadata.level() <= self.level {
            let level_filters = get_level_filters().read();
            let mut target = metadata.target();
            while !target.is_empty() {
                if let Some(level) = level_filters.get(target) {
                    return metadata.level() <= *level;
                }
                target = match target.rsplit_once("::") {
                    Some((left, _)) => left,
                    None => "",
                }
            }
            true
        } else {
            false
        }
    }

    #[cfg(feature = "tokio")]
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) { return; }

        #[cfg(feature = "time")]
        unsafe { time::util::local_offset::set_soundness(time::util::local_offset::Soundness::Unsound); }
        #[cfg(feature = "time")]
        let now = time::OffsetDateTime::now_local().unwrap().format(&self.dt_fmt).unwrap();
        #[cfg(feature = "chrono")]
        let now = chrono::Local::now().format(&self.dt_fmt);

        let mut msg = get_msg_from_cache();

        // 日志条目格式化
        if self.level >= log::LevelFilter::Debug {
            write!(&mut msg, "[\x1b[36m{now}\x1b[0m] [{}{:5}\x1b[0m] [{}::{}] - {}\n",
                    level_color(record.level()),
                    record.level(),
                    record.target(),
                    record.line().unwrap_or(0),
                    record.args()).unwrap();
        } else {
            write!(&mut msg, "[{now}] [{:5}] - {}\n", record.level(), record.args()).unwrap();
        };

        // 将需要输出的日志条目加入队列尾部，并返回是否已有任务正在处理日志
        let has_task = {
            let mut msg_queue = MSG_QUEUE.lock();
            msg_queue.0.push_back(msg);
            let has_task = msg_queue.1;
            if !has_task {
                msg_queue.1 = true;
            }
            has_task
        };
        // 如果没有任务正在处理日志，则创建一个任务
        if !has_task {
            tokio::spawn(write_async());
        }
    }

    #[cfg(not(feature = "tokio"))]
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) { return; }

        #[cfg(feature = "time")]
        unsafe { time::util::local_offset::set_soundness(time::util::local_offset::Soundness::Unsound); }
        #[cfg(feature = "time")]
        let now = time::OffsetDateTime::now_local().unwrap().format(&self.dt_fmt).unwrap();
        #[cfg(feature = "chrono")]
        let now = chrono::Local::now().format(&self.dt_fmt);

        let mut msg = get_msg_from_cache();

        // 日志条目格式化
        if self.level >= log::LevelFilter::Debug {
            write!(&mut msg, "[\x1b[36m{now}\x1b[0m] [{}{:5}\x1b[0m] [{}::{}] - {}\n",
                    level_color(record.level()),
                    record.level(),
                    record.target(),
                    record.line().unwrap_or(0),
                    record.args()).unwrap();
        } else {
            write!(&mut msg, "[{now}] [{:5}] - {}\n", record.level(), record.args()).unwrap();
        };

        if let Some(ref sender) = self.logger_data.lock().sender {
            // 采用独立的单线程写入日志的方式，向channel发送要写入的日志消息即可
            sender.send(AsyncLogType::Message(msg)).unwrap();
        } else {
            // 同步写入模式
            self.write(&msg);
            msg.truncate(0);
            let _ = get_cache_strs().push(msg);
        }
    }

    #[cfg(feature = "tokio")]
    fn flush(&self) {
        tokio::spawn(async move {
            let async_looger = get_async_logger();
            let mut logger_data = async_looger.logger_data.lock().await;
            if let Some(ref mut console) =  logger_data.console {
                console.flush().await.expect("flush log to console failed");
            }
            if let Some(ref mut fileout) = logger_data.fileout {
                fileout.flush().await.expect("flush log to file failed");
            }
        });
    }

    #[cfg(not(feature = "tokio"))]
    fn flush(&self) {
        let logger_data = self.logger_data.lock();
        if let Some(ref sender) = logger_data.sender {
            sender.send(AsyncLogType::Flush).unwrap();
        } else {
            drop(logger_data);
            self.flush_inner();
        }
    }

}


impl Builder {
    #[inline]
    pub fn new() -> Self {
        Self {
            level:          log::LevelFilter::Info,
            log_file:       String::new(),
            log_file_max:   10 * 1024 * 1024,
            use_console:    true,
            use_async:      true,
            plugin:         None,
        }
    }

    #[inline]
    pub fn builder(self) -> Result<()> {
        init_log_with_plugin(self.level, self.log_file, self.log_file_max,
                self.use_console, self.use_async, self.plugin)
    }

    #[inline]
    pub fn level(mut self, level: log::LevelFilter) -> Self {
        self.level = level;
        self
    }

    #[inline]
    pub fn log_file(mut self, log_file: String) -> Self {
        self.log_file = log_file;
        self
    }

    #[inline]
    pub fn log_file_max(mut self, log_file_max: u32) -> Self {
        self.log_file_max = log_file_max;
        self
    }

    #[inline]
    pub fn use_console(mut self, use_console: bool) -> Self {
        self.use_console = use_console;
        self
    }

    #[inline]
    pub fn use_async(mut self, use_async: bool) -> Self {
        self.use_async = use_async;
        self
    }

    #[inline]
    pub fn level_str(mut self, level: &str) -> Result<Self> {
        self.level = parse_level(level)?;
        Ok(self)
    }

    #[inline]
    pub fn log_file_max_str(mut self, log_file_max: &str) -> Result<Self> {
        self.log_file_max = parse_size(log_file_max)?;
        Ok(self)
    }

    pub fn plugin<T: std::io::Write + Send + 'static>(mut self, plugin: T) -> Self {
        self.plugin = Some(Box::new(plugin));
        self
    }
}


impl<'a> SkipAnsiColorIter<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let find_len = if data.len() > 3 {
            data.len() - 3
        } else {
            0
        };

        SkipAnsiColorIter {
            data,
            pos: 0,
            find_len,
        }
    }
}

impl<'a> Iterator for SkipAnsiColorIter<'a> {
    type Item = &'a [u8];

    #[inline]
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
                let n = if *data.get_unchecked(pos + 3) == b'm' { 4 } else { 5 };
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


#[cfg(not(feature = "tokio"))]
fn write_text(w: &mut LineWriter<File>, msg: &[u8]) -> std::io::Result<usize> {
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


#[cfg(feature = "tokio")]
async fn write_async() {

    let mut logger_data = get_async_logger().logger_data.lock().await;
    let async_logger = get_async_logger();

    // 首次使用，初始化控制台
    if logger_data.use_console && logger_data.console.is_none() {
        logger_data.console = Some(BufWriter::new(tokio::io::stdout()));
    }

    // 首次使用，初始化日志文件
    if logger_data.use_file && logger_data.fileout.is_none() {
        let (f, size) = open_log_file(&get_async_logger().log_file, true).await;
        logger_data.fileout = Some(f);
        logger_data.log_size = size;
    }


    // 循环读取消息队列并输出，当消息队列为空时，设置任务终止标志并终止任务
    loop {
        let mut msg = {
            let mut msg_queue = MSG_QUEUE.lock();
            match msg_queue.0.pop_front() {
                Some(msg) => msg,
                None => {
                    // 设置任务终止标志, 并结束异步任务
                    msg_queue.1 = false;
                    if msg_queue.0.capacity() > 256 {
                        msg_queue.0.shrink_to(256);
                    }
                    return;
                }
            }
        };

        // 如果启用了控制台输出，则写入控制台
        if let Some(ref mut console) = logger_data.console {
            if console.write_all(&msg).await.is_ok() {
                let l = msg.len();
                if l > 0 {
                    let c = msg[l - 1];
                    if c == b'\n' || c == b'\r' {
                        let _ = console.flush().await;
                    }
                }
            }
        }

        // 判断日志长度是否到达最大限制，如果到了，需要备份当前日志文件并重新创建新的日志文件
        if logger_data.log_size > async_logger.max_size {
            let has_file =  match logger_data.fileout {
                Some(ref mut fileout) => {
                    // 刷新缓存并关闭日志文件
                    fileout.flush().await.expect("flush log file fail");
                    true
                }
                None => false
            };

            if has_file {
                logger_data.fileout.take();

                // 之所以把关闭文件和重新创建文件分开写，是因为rust限制了可变借用(fileout)只允许1次
                // 删除已有备份，并重命名现有文件为备份文件
                let bak = format!("{}.bak", async_logger.log_file);
                let _ = tokio::fs::remove_file(&bak).await;
                tokio::fs::rename(&async_logger.log_file, &bak).await.expect("rename log file to bak log fail");

                logger_data.fileout = Some(open_log_file(&async_logger.log_file, false).await.0);
                logger_data.log_size = 0;
            }
        }

        if let Some(ref mut fileout) = logger_data.fileout {
            let s = write_text(fileout, &msg).await.expect("write log to file fail");
            logger_data.log_size += s as u32;
        }

        if let Some(ref mut plugin) = logger_data.plugin {
            plugin.write_all(&msg).expect("write log to plugin fail");
        }

        msg.truncate(0);
        let _ = get_cache_strs().push(msg);
    }
}

#[cfg(feature = "tokio")]
async fn write_text(w: &mut BufWriter<File>, msg: &[u8]) -> std::io::Result<usize> {
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

#[cfg(feature = "tokio")]
async fn open_log_file(log_file: &str, append: bool) -> (BufWriter<File>, u32) {
    let f = tokio::fs::OpenOptions::new()
        .append(append)
        .write(true)
        .create(true)
        .open(log_file)
        .await
        .expect("reopen log file fail");

    let size = if append {
        tokio::fs::metadata(log_file).await.map_or_else(|_| 0, |m| m.len())
    } else {
        0
    };
    return (BufWriter::new(f), size as u32);
}

#[cfg(feature = "tokio")]
fn get_async_logger() -> &'static AsyncLogger {
    unsafe {
        match &ASYNC_LOGGER {
            Some(logger) => *logger,
            None => std::hint::unreachable_unchecked()
        }
    }
}


// 返回日志级别对应的ansi颜色
fn level_color(level: log::Level) -> &'static str {
    // const RESET:    &str = "\x1b[0m";
    // const BLACK:    &str = "\x1b[30m";
    const RED:      &str = "\x1b[31m";
    const GREEN:    &str = "\x1b[32m";
    const YELLOW:   &str = "\x1b[33m";
    const BLUE:     &str = "\x1b[34m";
    const MAGENTA:  &str = "\x1b[35m";
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

fn get_cache_strs() -> &'static ArrayQueue<Vec<u8>> {
    CACHE_STRS.get_or_init(|| {
        ArrayQueue::new(CACHE_STR_ARRAY_SIZE)
    })
}

fn get_msg_from_cache() -> Vec<u8> {
    match get_cache_strs().pop() {
        Some(v) => v,
        None => Vec::with_capacity(CACHE_STR_INIT_SIZE)
    }
}

fn get_level_filters() -> &'static RwLock<HashMap<String, log::LevelFilter>> {
    LEVEL_FILTERS.get_or_init(|| {
        RwLock::new(HashMap::new())
    })
}

#[cfg(debug_assertions)]
fn debug_check_init() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INITED: AtomicBool = AtomicBool::new(false);

    if let Err(true) = INITED.compare_exchange(false, true, Ordering::Release, Ordering::Relaxed) {
        panic!("init_log must run once!");
    }
}
