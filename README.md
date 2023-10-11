# asynclog -- simple async log library
简单异步日志库，基于log库

---
#### 项目地址
<https://github.com/kivensoft/asynclog_rs>

###### 第三方依赖
* log
* parking_lot
* time [optional]
* chrono [optional]

---
###### 添加依赖
`cargo add --git https://github.com/kivensoft/asynclog_rs asynclog`
###### 使用
```rust
use asynclog;
use log;

fn main() {
	let log_level = asynclog::parse_level("debug").unwrap();
	let log_max = asynclog::parse_size("100k").unwrap();

	asynclog::init_log(log_level, "app.log".to_string(), log_max, true, true).unwrap();

	log::trace!("hello {}!", "trace");
	log::debug!("hello {}!", "debug");
	log::info!("hello {}!", "info");
	log::error!("hello {}!", "error");
}
```
