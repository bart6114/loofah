use std::path::PathBuf;

pub struct Tracing<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Tracing<'a, R, M> {
    pub fn logs_dir(&self) -> Result<PathBuf, crate::Error> {
        let logs_dir = self
            .manager
            .path()
            .app_log_dir()
            .map_err(|e| crate::Error::PathResolver(e.to_string()))?;
        std::fs::create_dir_all(&logs_dir)
            .map_err(|e| crate::Error::PathResolver(format!("create logs dir: {}", e)))?;
        Ok(logs_dir)
    }

    pub fn do_log(&self, level: Level, data: Vec<serde_json::Value>) -> Result<(), crate::Error> {
        match level {
            Level::Trace => {
                tracing::trace!("{:?}", data);
            }
            Level::Debug => {
                tracing::debug!("{:?}", data);
            }
            Level::Info => {
                tracing::info!("{:?}", data);
            }
            Level::Warn => {
                tracing::warn!("{:?}", data);
            }
            Level::Error => {
                tracing::error!("{:?}", data);
            }
        }
        Ok(())
    }

    pub fn log_content(&self) -> Result<Option<String>, crate::Error> {
        let logs_dir = self.logs_dir()?;
        const TARGET_LINES: usize = 300;
        const MAX_ROTATED_FILES: usize = 5;

        let log_files: Vec<_> = std::iter::once(logs_dir.join("app.log"))
            .chain((1..=MAX_ROTATED_FILES).map(|i| logs_dir.join(format!("app.log.{}", i))))
            .collect();

        let mut collected: Vec<String> = Vec::new();

        for log_path in &log_files {
            if collected.len() >= TARGET_LINES {
                break;
            }

            if let Ok(content) = std::fs::read_to_string(log_path) {
                let lines_needed = TARGET_LINES.saturating_sub(collected.len());
                let lines = tail_lines(&content, lines_needed);
                let mut new_collected = lines;
                new_collected.extend(collected);
                collected = new_collected;
            }
        }

        if collected.is_empty() {
            return Ok(None);
        }

        let start = collected.len().saturating_sub(TARGET_LINES);
        Ok(Some(collected[start..].join("\n")))
    }
}

fn tail_lines(content: &str, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }

    let mut lines: Vec<_> = content
        .lines()
        .rev()
        .take(max_lines)
        .map(str::to_string)
        .collect();
    lines.reverse();
    lines
}

pub trait TracingPluginExt<R: tauri::Runtime> {
    fn tracing(&self) -> Tracing<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}

impl<R: tauri::Runtime, T: tauri::Manager<R>> TracingPluginExt<R> for T {
    fn tracing(&self) -> Tracing<'_, R, Self>
    where
        Self: Sized,
    {
        Tracing {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, specta::Type)]
pub enum Level {
    #[serde(rename = "TRACE")]
    Trace,
    #[serde(rename = "DEBUG")]
    Debug,
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERROR")]
    Error,
}

pub const JS_INIT_SCRIPT: &str = r#"
(function() {
    function initConsoleOverride() {
        if (typeof window.__TAURI__ === 'undefined' || 
            typeof window.__TAURI__.core === 'undefined' ||
            typeof window.__TAURI__.core.invoke === 'undefined') {
            setTimeout(initConsoleOverride, 10);
            return;
        }
        
        const originalLog = console.log.bind(console);
        const originalDebug = console.debug.bind(console);
        const originalInfo = console.info.bind(console);
        const originalWarn = console.warn.bind(console);
        const originalError = console.error.bind(console);
        
        const invoke = window.__TAURI__.core.invoke;
        function normalize(args) {
            // Reserve room for truncation markers and the IPC envelope.
            let remaining = 64 * 1024 - 4096;
            const ancestors = new Set();
            const truncated = '[Truncated: text budget]';
            function text(value, limit = remaining) {
                const marker = '[Truncated]';
                const budget = Math.max(0, Math.min(limit, remaining) - marker.length - 2);
                let result = '';
                let bytes = 0;
                for (let char of value) {
                    const code = char.codePointAt(0);
                    const invalidUnicode = code >= 0xD800 && code <= 0xDFFF;
                    if (invalidUnicode) char = '[Invalid Unicode]';
                    const size = invalidUnicode ? char.length
                        : code < 32 ? ([8, 9, 10, 12, 13].includes(code) ? 2 : 6)
                        : code === 34 || code === 92 ? 2
                        : code < 128 ? 1 : code < 2048 ? 2
                        : code < 65536 ? 3 : 4;
                    if (bytes + size > budget) {
                        result += marker;
                        bytes += marker.length;
                        break;
                    }
                    result += char;
                    bytes += size;
                }
                remaining -= bytes + 2;
                return result;
            }
            function isError(value) {
                try {
                    return value instanceof Error || Object.prototype.toString.call(value) === '[object Error]';
                } catch (_) { return false; }
            }
            function property(object, key, depth, limit) {
                try { return visit(object[key], depth, limit); }
                catch (_) { return text('[Inaccessible property]'); }
            }
            function visit(value, depth, limit) {
                if (remaining < 128) return truncated;
                if (typeof value === 'string') return text(value, limit);
                if (value === null || typeof value === 'boolean' || typeof value === 'number') {
                    remaining -= 24;
                    return value;
                }
                if (typeof value === 'bigint') return text('[BigInt]');
                if (typeof value !== 'object') return text('[Unsupported: ' + typeof value + ']');
                if (ancestors.has(value)) return text('[Circular]');
                if (depth >= 8) return text('[Truncated: depth]');
                ancestors.add(value);
                try {
                    if (value instanceof Date) return text(value.toISOString());
                    remaining -= 2;
                    if (Array.isArray(value)) {
                        const result = [];
                        const length = value.length;
                        for (let i = 0; i < Math.min(length, 100); i++) {
                            if (remaining < 128) { result.push(truncated); return result; }
                            remaining--;
                            result.push(property(value, i, depth + 1));
                        }
                        if (length > 100) result.push(text('[Truncated: entries]'));
                        return result;
                    }
                    const result = Object.create(null);
                    const error = isError(value);
                    let count = 0;
                    function add(key, limit) {
                        if (Object.prototype.hasOwnProperty.call(result, key)) return true;
                        if (remaining < 128 || count >= 100) {
                            result['[Truncated]'] = count >= 100 ? '[Truncated: entries]' : truncated;
                            return false;
                        }
                        const normalizedKey = text(key, 1024);
                        remaining -= 2;
                        result[normalizedKey] = property(value, key, depth + 1, limit);
                        count++;
                        return true;
                    }
                    if (error) {
                        add('name', 1024);
                        add('message', 8192);
                        add('stack', 24576);
                        if ('cause' in value) add('cause');
                        if ('errors' in value) add('errors');
                    }
                    for (const key of Object.keys(value)) {
                        if (!add(key)) break;
                    }
                    return result;
                } catch (_) {
                    return text('[Inaccessible object]');
                } finally {
                    ancestors.delete(value);
                }
            }
            const count = Math.min(args.length, 100);
            const result = Array(count);
            // Error diagnostics get space before large contextual arguments,
            // while the resulting payload retains the original argument order.
            for (let i = 0; i < count; i++) {
                try {
                    if (isError(args[i])) result[i] = visit(args[i], 0);
                } catch (_) {}
            }
            for (let i = 0; i < count; i++) {
                if (!(i in result)) result[i] = visit(args[i], 0);
            }
            if (args.length > count) result.push('[Truncated: entries]');
            return result;
        }
        const log = (level, ...args) => {
            try {
                const data = normalize(args);
                Promise.resolve(invoke('plugin:tracing|do_log', { level, data })).catch(() => {});
            } catch (_) {}
        };
        
        console.log = (...args) => { originalLog(...args); log('INFO', ...args); };
        console.debug = (...args) => { originalDebug(...args); log('DEBUG', ...args); };
        console.info = (...args) => { originalInfo(...args); log('INFO', ...args); };
        console.warn = (...args) => { originalWarn(...args); log('WARN', ...args); };
        console.error = (...args) => { originalError(...args); log('ERROR', ...args); };
    }
    
    initConsoleOverride();
})();
"#;

#[cfg(test)]
mod tests {
    use rquickjs::{Context, Runtime};

    #[test]
    fn tail_lines_returns_recent_lines() {
        let content = "line 1\nline 2\nline 3\nline 4";

        assert_eq!(
            super::tail_lines(content, 2),
            vec!["line 3".to_string(), "line 4".to_string()]
        );
    }

    #[test]
    fn tail_lines_handles_zero_limit() {
        assert!(super::tail_lines("line 1\nline 2", 0).is_empty());
    }

    fn payloads(script: &str) -> serde_json::Value {
        let runtime = Runtime::new().unwrap();
        let unhandled = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        let tracker = unhandled.clone();
        runtime.set_host_promise_rejection_tracker(Some(Box::new(move |_, _, _, handled| {
            tracker.fetch_add(
                if handled { -1 } else { 1 },
                std::sync::atomic::Ordering::SeqCst,
            );
        })));
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| {
            ctx.eval::<(), _>(r#"
                globalThis.window = globalThis;
                globalThis.payloads = [];
                globalThis.originals = [];
                globalThis.rejectIpc = false;
                globalThis.throwIpc = false;
                globalThis.console = {};
                for (const name of ['log', 'debug', 'info', 'warn', 'error']) {
                    console[name] = (...args) => originals.push(args);
                }
                globalThis.__TAURI__ = { core: { invoke: (command, payload) => {
                    if (throwIpc) throw new Error('IPC unavailable');
                    payloads.push(JSON.parse(JSON.stringify(payload)));
                    return rejectIpc ? Promise.reject(new Error('IPC rejected')) : Promise.resolve();
                } } };
            "#).unwrap();
            ctx.eval::<(), _>(super::JS_INIT_SCRIPT).unwrap();
            ctx.eval::<(), _>(script).unwrap();
        });
        while runtime.execute_pending_job().unwrap() {}
        assert_eq!(unhandled.load(std::sync::atomic::Ordering::SeqCst), 0);
        context.with(|ctx| {
            serde_json::from_str(&ctx.eval::<String, _>("JSON.stringify(payloads)").unwrap())
                .unwrap()
        })
    }

    #[test]
    fn errors_preserve_identity_causes_aggregates_and_custom_fields() {
        let values = payloads(
            r#"
            class CustomError extends Error {}
            const cause = new TypeError('root cause');
            const error = new CustomError('outer', { cause });
            error.code = 42;
            error.detail = { nested: new RangeError('nested') };
            console.error('prefix', error, { error });
            if (originals[0][1] !== error || originals[0][2].error !== error) throw new Error('original arguments changed');
            console.warn(new AggregateError([error, cause], 'aggregate'));
        "#,
        );
        let error = &values[0]["data"][1];
        assert_eq!(error["message"], "outer");
        assert_eq!(error["code"], 42);
        assert!(error["stack"].as_str().unwrap().contains("eval"));
        assert_eq!(error["cause"]["name"], "TypeError");
        assert_eq!(error["cause"]["message"], "root cause");
        assert_eq!(error["detail"]["nested"]["name"], "RangeError");
        assert_eq!(values[0]["data"][2]["error"], *error);
        assert_eq!(values[1]["data"][0]["name"], "AggregateError");
        assert_eq!(values[1]["data"][0]["errors"][0], *error);
    }

    #[test]
    fn ordinary_arguments_and_levels_are_preserved() {
        let values = payloads(
            r#"
            for (const name of ['log', 'debug', 'info', 'warn', 'error']) {
                console[name]('hello', 42, true, null, { key: ['value', 3] });
            }
        "#,
        );
        for (index, level) in ["INFO", "DEBUG", "INFO", "WARN", "ERROR"]
            .iter()
            .enumerate()
        {
            assert_eq!(values[index]["level"], *level);
            assert_eq!(
                values[index]["data"],
                serde_json::json!(["hello", 42, true, null, {"key": ["value", 3]}])
            );
        }
    }

    #[test]
    fn circular_bigint_and_inaccessible_values_are_explicit() {
        let values = payloads(
            r#"
            const value = { big: 123n };
            value.self = value;
            Object.defineProperty(value, 'broken', { enumerable: true, get() { throw new Error('getter'); } });
            const error = new Error('cyclic');
            error.cause = error;
            console.error(value, error, new Proxy({}, { ownKeys() { throw new Error('proxy'); } }));
        "#,
        );
        assert_eq!(values[0]["data"][0]["self"], "[Circular]");
        assert_eq!(values[0]["data"][0]["big"], "[BigInt]");
        assert_eq!(values[0]["data"][0]["broken"], "[Inaccessible property]");
        assert_eq!(values[0]["data"][1]["cause"], "[Circular]");
        assert_eq!(values[0]["data"][2], "[Inaccessible object]");
    }

    #[test]
    fn traversal_and_encoded_payload_size_are_bounded() {
        let values = payloads(
            r#"
            let deep = {};
            for (let i = 0; i < 20; i++) deep = { child: deep };
            console.log(deep);
            console.log(Array(150).fill('entry'));
            console.log(Object.fromEntries(Array.from({length: 150}, (_, i) => ['key' + i, i])));
            const huge = new Error('huge'.repeat(20000));
            huge.stack = 'at eval (test.js:1)';
            console.error(huge);
            console.log(Array(100).fill('\u0000\\😀'.repeat(20000)));
            console.log(Array(100).fill({ a: Array(100).fill('x') }));
            console.log(String.fromCharCode(0, 92, 0xD800).repeat(50000));
        "#,
        );
        assert!(values[0].to_string().contains("[Truncated: depth]"));
        assert_eq!(values[1]["data"][0][100], "[Truncated: entries]");
        assert_eq!(values[2]["data"][0]["[Truncated]"], "[Truncated: entries]");
        assert_eq!(values[3]["data"][0]["name"], "Error");
        assert!(
            values[3]["data"][0]["message"]
                .as_str()
                .unwrap()
                .contains("[Truncated]")
        );
        assert!(
            values[3]["data"][0]["stack"]
                .as_str()
                .unwrap()
                .contains("eval")
        );
        for value in values.as_array().unwrap() {
            assert!(serde_json::to_vec(value).unwrap().len() <= 64 * 1024);
        }
    }

    #[test]
    fn error_details_take_priority_over_large_context_arguments() {
        let values = payloads(
            r#"
            const error = new Error('essential diagnostic');
            error.stack = 'essential stack';
            console.error('context'.repeat(20000), error);
        "#,
        );
        assert_eq!(values[0]["data"][1]["message"], "essential diagnostic");
        assert_eq!(values[0]["data"][1]["stack"], "essential stack");
        assert!(
            values[0]["data"][0]
                .as_str()
                .unwrap()
                .ends_with("[Truncated]")
        );
        assert!(serde_json::to_vec(&values[0]).unwrap().len() <= 64 * 1024);
    }

    #[test]
    fn ipc_failure_does_not_throw_or_reenter_console() {
        let values = payloads(
            r#"
            rejectIpc = true;
            console.error(new Error('keep original'));
            throwIpc = true;
            console.log('still running');
            if (originals.length !== 2) throw new Error('console recursion');
        "#,
        );
        assert_eq!(values.as_array().unwrap().len(), 1);
    }

    #[test]
    fn serialized_error_messages_and_stacks_are_redacted() {
        use std::io::Write;
        let values = payloads(
            r#"
            const error = new Error('user@example.com at 192.168.1.1');
            error.stack = 'at user@example.com (192.168.1.1)';
            console.error(error);
        "#,
        );
        let mut output = Vec::new();
        {
            let mut writer = crate::redaction::RedactingWriter::new(&mut output);
            writeln!(writer, "{:?}", values[0]["data"]).unwrap();
        }
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("message"));
        assert!(text.contains("stack"));
        assert_eq!(text.matches("[EMAIL_REDACTED]").count(), 2);
        assert_eq!(text.matches("[IP_REDACTED]").count(), 2);
        assert!(!text.contains("user@example.com"));
        assert!(!text.contains("192.168.1.1"));
    }
}
