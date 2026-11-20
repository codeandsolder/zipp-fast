/* ZIPP PHP runtime — PDO, SQLite3, and mysqli backed by sandboxed host SQL bridge (`__zipp_php_sql` -> `app.sql.*`). Apache-2.0. */
(function (R) {
    "use strict";
    const rt = R.__rt;
    const reg = rt.regFn;
    const defC = function (k, v) { rt.consts.set(k, v); };

    // ---- Constants ----
    defC("SQLITE3_ASSOC", 1n);
    defC("SQLITE3_NUM", 2n);
    defC("SQLITE3_BOTH", 3n);
    defC("SQLITE3_INTEGER", 1n);
    defC("SQLITE3_FLOAT", 2n);
    defC("SQLITE3_TEXT", 3n);
    defC("SQLITE3_BLOB", 4n);
    defC("SQLITE3_NULL", 5n);
    defC("SQLITE3_OPEN_READONLY", 1n);
    defC("SQLITE3_OPEN_READWRITE", 2n);
    defC("SQLITE3_OPEN_CREATE", 4n);

    defC("MYSQLI_ASSOC", 1n);
    defC("MYSQLI_NUM", 2n);
    defC("MYSQLI_BOTH", 3n);
    defC("MYSQLI_STORE_RESULT", 0n);
    defC("MYSQLI_USE_RESULT", 1n);
    defC("MYSQLI_REPORT_OFF", 0n);
    defC("MYSQLI_REPORT_ERROR", 1n);
    defC("MYSQLI_REPORT_STRICT", 2n);
    defC("MYSQLI_REPORT_INDEX", 4n);
    defC("MYSQLI_REPORT_ALL", 255n);

    function sqlCall(op, a, b, c) {
        if (typeof __zipp_php_sql !== "function") {
            return { error: "SQL driver unavailable: no sandboxed SQL plugin (app.sql.*) attached" };
        }
        const raw = __zipp_php_sql(
            String(op),
            a === undefined || a === null ? "" : String(a),
            b === undefined || b === null ? "" : String(b),
            c === undefined || c === null ? "" : String(c)
        );
        if (typeof raw === "string") {
            try { return JSON.parse(raw); } catch (_) { return { error: raw }; }
        }
        return { error: "invalid SQL host bridge reply" };
    }

    function encodePhpParam(v) {
        const d = rt.deref(v);
        if (d === null || d === undefined) return null;
        if (typeof d === "boolean") return d ? 1 : 0;
        if (typeof d === "bigint") return Number(d);
        if (typeof d === "number") return Number.isFinite(d) ? d : null;
        return rt.toStr(d);
    }

    function decodeSqlVal(v) {
        if (v === null || v === undefined) return null;
        if (typeof v === "boolean") return v ? 1n : 0n;
        if (typeof v === "number") return Number.isInteger(v) ? BigInt(v) : v;
        if (typeof v === "object" && typeof v.hex === "string") {
            let out = "";
            for (let i = 0; i + 1 < v.hex.length; i += 2) {
                out += String.fromCharCode(parseInt(v.hex.slice(i, i + 2), 16));
            }
            return out;
        }
        return String(v);
    }

    function makePhpRow(columns, rawRow, mode) {
        if (mode === 5) { // FETCH_OBJ
            const obj = rt.allocInstance(rt.classes.get("stdclass"));
            for (let i = 0; i < columns.length; i++) {
                obj.props[columns[i]] = decodeSqlVal(rawRow[i]);
            }
            return obj;
        }
        if (mode === 7) { // FETCH_COLUMN
            return rawRow.length > 0 ? decodeSqlVal(rawRow[0]) : false;
        }
        const arr = rt.makeArr();
        const wantAssoc = mode === 2 || mode === 4 || mode === 1 || mode === 3;
        const wantNum = mode === 3 || mode === 4 || mode === 22; // 4=PDO::FETCH_BOTH, 3=PDO::FETCH_NUM or MYSQLI_BOTH
        // Normalize mode flags cleanly via helper callers instead of overloading numbers
        for (let i = 0; i < columns.length; i++) {
            const val = decodeSqlVal(rawRow[i]);
            if (wantAssoc) rt.arrSetInPlace(arr, columns[i], val);
            if (wantNum) rt.arrSetInPlace(arr, BigInt(i), val);
        }
        return arr;
    }

    function buildRowByStyle(columns, rawRow, style) {
        // style: "assoc" | "num" | "both" | "obj" | "col"
        if (style === "obj") {
            const obj = rt.allocInstance(rt.classes.get("stdclass"));
            for (let i = 0; i < columns.length; i++) {
                obj.props[columns[i]] = decodeSqlVal(rawRow[i]);
            }
            return obj;
        }
        if (style === "col") {
            return rawRow.length > 0 ? decodeSqlVal(rawRow[0]) : false;
        }
        const arr = rt.makeArr();
        for (let i = 0; i < columns.length; i++) {
            const val = decodeSqlVal(rawRow[i]);
            if (style === "assoc" || style === "both") rt.arrSetInPlace(arr, columns[i], val);
            if (style === "num" || style === "both") rt.arrSetInPlace(arr, BigInt(i), val);
        }
        return arr;
    }

    // ---- Lightweight MySQL-to-SQLite SQL compatibility translator (for mysqli / mysql: PDO) ----
    function convertMysqlEscapes(sql) {
        // Convert MySQL backslash-escaped single quotes inside '...' string literals into SQLite ''
        let out = "";
        let inSq = false;
        let inDq = false;
        for (let i = 0; i < sql.length; i++) {
            const ch = sql[i];
            if (inSq) {
                if (ch === "\\" && i + 1 < sql.length) {
                    const nxt = sql[i + 1];
                    if (nxt === "'") { out += "''"; i++; continue; }
                    if (nxt === "\\") { out += "\\"; i++; continue; }
                    if (nxt === "n") { out += "\n"; i++; continue; }
                    if (nxt === "r") { out += "\r"; i++; continue; }
                    if (nxt === "t") { out += "\t"; i++; continue; }
                    if (nxt === "0") { out += "\0"; i++; continue; }
                    out += nxt; i++; continue;
                }
                if (ch === "'" && sql[i + 1] === "'") { out += "''"; i++; continue; }
                if (ch === "'") inSq = false;
                out += ch;
            } else if (inDq) {
                if (ch === "\\" && i + 1 < sql.length) { out += sql[i + 1]; i++; continue; }
                if (ch === '"') inDq = false;
                out += ch;
            } else {
                if (ch === "'") { inSq = true; out += ch; }
                else if (ch === '"') { inDq = true; out += ch; }
                else if (ch === "`") { out += '"'; }
                else out += ch;
            }
        }
        return out;
    }

    function translateMysqlToSqlite(rawSql) {
        const trimmed = String(rawSql || "").trim();
        if (!trimmed) return { noop: true };
        const upper = trimmed.toUpperCase();

        // Session / charset / mode / lock no-ops
        if (/^SET\s+(?:TIME_ZONE|NAMES|CHARACTER\s+SET|CHARSET|SQL_MODE|SESSION|GLOBAL|FOREIGN_KEY_CHECKS|UNIQUE_CHECKS|AUTOCOMMIT|@)/i.test(trimmed)) {
            return { noop: true };
        }
        if (/^(?:LOCK|UNLOCK)\s+TABLES\b/i.test(trimmed)) {
            return { noop: true };
        }
        if (/^SELECT\s+VERSION\s*\(\s*\)(?:\s+AS\s+["'`]?(\w+)["'`]?)?\s*;?$/i.test(trimmed)) {
            const m = /^SELECT\s+VERSION\s*\(\s*\)(?:\s+AS\s+["'`]?(\w+)["'`]?)?\s*;?$/i.exec(trimmed);
            const alias = (m && m[1]) ? m[1] : "VERSION()";
            return { synthetic: { columns: [alias], rows: [["8.0.36-zipp-xdb"]], rows_affected: 0, last_insert_rowid: 0 } };
        }
        if (/^SHOW\s+TABLES(?:\s+LIKE\s+['"]([^'"]+)['"])?\s*;?$/i.test(trimmed)) {
            const m = /^SHOW\s+TABLES(?:\s+LIKE\s+['"]([^'"]+)['"])?\s*;?$/i.exec(trimmed);
            if (m && m[1]) {
                return { sql: "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '" + m[1].replace(/'/g, "''") + "' ORDER BY name" };
            }
            return { sql: "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name" };
        }
        if (/^(?:DESCRIBE|DESC|SHOW\s+COLUMNS\s+FROM)\s+["'`]?(\w+)["'`]?\s*;?$/i.test(trimmed)) {
            const m = /^(?:DESCRIBE|DESC|SHOW\s+COLUMNS\s+FROM)\s+["'`]?(\w+)["'`]?\s*;?$/i.exec(trimmed);
            return { sql: "PRAGMA table_info(" + m[1] + ")" };
        }
        if (upper === "START TRANSACTION" || upper === "START TRANSACTION;") {
            return { sql: "BEGIN TRANSACTION" };
        }

        let sql = convertMysqlEscapes(trimmed);

        // Common MySQL functions -> SQLite equivalents
        sql = sql.replace(/\bNOW\s*\(\s*\)/gi, "datetime('now')");
        sql = sql.replace(/\bCURRENT_TIMESTAMP\s*\(\s*\)/gi, "datetime('now')");
        sql = sql.replace(/\bUNIX_TIMESTAMP\s*\(\s*\)/gi, "CAST(strftime('%s','now') AS INTEGER)");
        sql = sql.replace(/\bRAND\s*\(\s*\)/gi, "ABS(RANDOM() / 9223372036854775807.0)");
        // LIMIT offset, count -> LIMIT count OFFSET offset
        sql = sql.replace(/\bLIMIT\s+(\d+)\s*,\s*(\d+)\b/gi, "LIMIT $2 OFFSET $1");

        // CREATE TABLE MySQL DDL cleanup
        if (/^CREATE\s+TABLE\b/i.test(sql)) {
            sql = sql.replace(/\b(?:TINY|SMALL|MEDIUM|BIG)?INT(?:\s*\(\s*\d+\s*\))?\s+UNSIGNED\b/gi, "INTEGER");
            sql = sql.replace(/\b(?:TINY|SMALL|MEDIUM|BIG)?INT\s*\(\s*\d+\s*\)/gi, "INTEGER");
            sql = sql.replace(/\bUNSIGNED\b/gi, "");
            sql = sql.replace(/\bAUTO_INCREMENT\b/gi, "AUTOINCREMENT");
            sql = sql.replace(/\bENUM\s*\([^)]*\)/gi, "TEXT");
            sql = sql.replace(/\bSET\s*\([^)]*\)/gi, "TEXT");
            sql = sql.replace(/\b(?:TINY|MEDIUM|LONG)TEXT\b/gi, "TEXT");
            sql = sql.replace(/\b(?:TINY|MEDIUM|LONG)BLOB\b/gi, "BLOB");
            sql = sql.replace(/\bDATETIME\b/gi, "TEXT");
            sql = sql.replace(/\bON\s+UPDATE\s+CURRENT_TIMESTAMP(?:\(\))?/gi, "");
            sql = sql.replace(/\bCHARACTER\s+SET\s+\w+/gi, "");
            sql = sql.replace(/\bCOLLATE\s+\w+/gi, "");
            sql = sql.replace(/\bCOMMENT\s+'(?:''|[^'])*'/gi, "");
            // Strip inline KEY / INDEX lines inside CREATE TABLE(...) that SQLite doesn't accept
            sql = sql.replace(/,\s*(?:UNIQUE\s+)?(?:FULLTEXT\s+)?(?:SPATIAL\s+)?(?:KEY|INDEX)\s+(?:"[^"]+"|\w+)?\s*\([^)]*\)/gi, "");
            // Strip trailing table options after final `)`
            sql = sql.replace(/\)\s*(?:ENGINE|DEFAULT|CHARSET|CHARACTER|COLLATE|AUTO_INCREMENT|COMMENT|ROW_FORMAT)\b[\s\S]*$/i, ")");
        }

        // INSERT IGNORE -> INSERT OR IGNORE
        sql = sql.replace(/\bINSERT\s+IGNORE\s+INTO\b/gi, "INSERT OR IGNORE INTO");

        return { sql: sql };
    }

    function isSelectLike(sql) {
        const s = String(sql || "").trim().toUpperCase();
        return s.startsWith("SELECT") || s.startsWith("WITH") || s.startsWith("PRAGMA") || s.startsWith("EXPLAIN") || s.startsWith("VALUES") || s.indexOf(" RETURNING ") >= 0;
    }

    function normalizeNamedParams(sql, params) {
        if (!params || typeof params !== "object" || Array.isArray(params)) {
            return { sql: sql, params: Array.isArray(params) ? params : [] };
        }
        // Convert named `:foo` params to positional `?` if associative array passed
        const positional = [];
        const rewritten = sql.replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, function (m, name) {
            if (Object.prototype.hasOwnProperty.call(params, ":" + name)) {
                positional.push(params[":" + name]);
                return "?";
            }
            if (Object.prototype.hasOwnProperty.call(params, name)) {
                positional.push(params[name]);
                return "?";
            }
            return m;
        });
        return { sql: rewritten, params: positional };
    }

    function runSql(connId, rawSql, params, mysqlCompat) {
        let sql = String(rawSql || "");
        if (mysqlCompat) {
            const tr = translateMysqlToSqlite(sql);
            if (tr.noop) return { columns: [], rows: [], rows_affected: 0, last_insert_rowid: 0 };
            if (tr.synthetic) return tr.synthetic;
            sql = tr.sql;
        }
        const norm = normalizeNamedParams(sql, params);
        const op = isSelectLike(norm.sql) ? "query" : "execute";
        return sqlCall(op, String(connId), norm.sql, JSON.stringify(norm.params));
    }

    // ---- PDOException & mysqli_sql_exception ----
    R.defClass({
        name: "PDOException", kind: "class", parent: "RuntimeException",
        props: [{ name: "errorInfo", vis: 0, hasDefault: true, defaultVal: null }],
        methods: []
    });
    R.defClass({
        name: "mysqli_sql_exception", kind: "class", parent: "RuntimeException",
        methods: []
    });

    function throwPdoError(msg, mode) {
        if (mode === 2n) { // ERRMODE_EXCEPTION
            rt.throwError("PDOException", String(msg));
        }
    }

    function pdoStyleOf(mode) {
        const m = Number(mode);
        if (m === 2) return "assoc";
        if (m === 3) return "num";
        if (m === 5) return "obj";
        if (m === 7) return "col";
        return "both"; // 4 = FETCH_BOTH
    }

    // ---- PDOStatement ----
    R.defClass({
        name: "PDOStatement", kind: "class",
        props: [{ name: "queryString", vis: 0, hasDefault: true, defaultVal: "" }],
        methods: [
            {
                name: "bindValue", vis: 0,
                builtin: function (param, value) {
                    if (!this.__params) this.__params = {};
                    const k = rt.deref(param);
                    if (typeof k === "bigint" || typeof k === "number") {
                        if (!this.__posParams) this.__posParams = [];
                        this.__posParams[Number(k) - 1] = encodePhpParam(value);
                    } else {
                        this.__params[rt.toStr(k)] = encodePhpParam(value);
                    }
                    return true;
                }
            },
            {
                name: "bindParam", vis: 0,
                builtin: function (param, value) {
                    if (!this.__params) this.__params = {};
                    const k = rt.deref(param);
                    if (typeof k === "bigint" || typeof k === "number") {
                        if (!this.__posParams) this.__posParams = [];
                        this.__posParams[Number(k) - 1] = encodePhpParam(value);
                    } else {
                        this.__params[rt.toStr(k)] = encodePhpParam(value);
                    }
                    return true;
                }
            },
            {
                name: "execute", vis: 0,
                builtin: function (inputParams) {
                    let params = [];
                    const ip = rt.deref(inputParams);
                    if (ip && ip.__php === 1) {
                        let isAssoc = false;
                        for (const k of ip.m.keys()) {
                            if (typeof k === "string") { isAssoc = true; break; }
                        }
                        if (isAssoc) {
                            params = {};
                            for (const [k, v] of ip.m) params[String(k)] = encodePhpParam(v);
                        } else {
                            for (const v of ip.m.values()) params.push(encodePhpParam(v));
                        }
                    } else if (this.__posParams && this.__posParams.length > 0) {
                        params = this.__posParams.slice();
                    } else if (this.__params && Object.keys(this.__params).length > 0) {
                        params = Object.assign({}, this.__params);
                    }
                    const res = runSql(this.__connId, this.__sql, params, !!this.__mysqlCompat);
                    if (res.error) {
                        this.__lastError = res.error;
                        if (this.__pdo) this.__pdo.__lastError = res.error;
                        throwPdoError(res.error, this.__errMode);
                        return false;
                    }
                    this.__columns = res.columns || [];
                    this.__rows = res.rows || [];
                    this.__cursor = 0;
                    this.__rowCount = res.rows_affected || this.__rows.length || 0;
                    if (this.__pdo && res.last_insert_rowid !== undefined && res.last_insert_rowid > 0) {
                        this.__pdo.__lastInsertId = res.last_insert_rowid;
                    }
                    return true;
                }
            },
            {
                name: "fetch", vis: 0,
                builtin: function (mode) {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return false;
                    const row = this.__rows[this.__cursor++];
                    const m = (mode !== undefined && rt.deref(mode) !== null) ? rt.toInt(mode) : (this.__fetchMode || 4n);
                    return buildRowByStyle(this.__columns || [], row, pdoStyleOf(m));
                }
            },
            {
                name: "fetchAll", vis: 0,
                builtin: function (mode, colIdx) {
                    const out = rt.makeArr();
                    if (!this.__rows) return out;
                    const m = (mode !== undefined && rt.deref(mode) !== null) ? rt.toInt(mode) : (this.__fetchMode || 4n);
                    const style = pdoStyleOf(m);
                    const ci = colIdx !== undefined && rt.deref(colIdx) !== null ? Number(rt.toInt(colIdx)) : 0;
                    while (this.__cursor < this.__rows.length) {
                        const row = this.__rows[this.__cursor++];
                        if (style === "col") {
                            rt.arrPushInPlace(out, ci < row.length ? decodeSqlVal(row[ci]) : null);
                        } else {
                            rt.arrPushInPlace(out, buildRowByStyle(this.__columns || [], row, style));
                        }
                    }
                    return out;
                }
            },
            {
                name: "fetchColumn", vis: 0,
                builtin: function (colNum) {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return false;
                    const row = this.__rows[this.__cursor++];
                    const idx = colNum !== undefined && rt.deref(colNum) !== null ? Number(rt.toInt(colNum)) : 0;
                    return idx >= 0 && idx < row.length ? decodeSqlVal(row[idx]) : false;
                }
            },
            {
                name: "fetchObject", vis: 0,
                builtin: function () {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return false;
                    const row = this.__rows[this.__cursor++];
                    return buildRowByStyle(this.__columns || [], row, "obj");
                }
            },
            { name: "rowCount", vis: 0, builtin: function () { return BigInt(this.__rowCount || 0); } },
            { name: "columnCount", vis: 0, builtin: function () { return BigInt((this.__columns || []).length); } },
            { name: "closeCursor", vis: 0, builtin: function () { this.__cursor = (this.__rows || []).length; return true; } },
            {
                name: "setFetchMode", vis: 0,
                builtin: function (mode) { this.__fetchMode = rt.toInt(mode); return true; }
            },
            {
                name: "errorCode", vis: 0,
                builtin: function () { return this.__lastError ? "HY000" : "00000"; }
            },
            {
                name: "errorInfo", vis: 0,
                builtin: function () {
                    const a = rt.makeArr();
                    rt.arrPushInPlace(a, this.__lastError ? "HY000" : "00000");
                    rt.arrPushInPlace(a, this.__lastError ? 1n : null);
                    rt.arrPushInPlace(a, this.__lastError || null);
                    return a;
                }
            },
        ]
    });

    // ---- PDO ----
    R.defClass({
        name: "PDO", kind: "class",
        consts: [
            { name: "FETCH_LAZY", val: 1n },
            { name: "FETCH_ASSOC", val: 2n },
            { name: "FETCH_NUM", val: 3n },
            { name: "FETCH_BOTH", val: 4n },
            { name: "FETCH_OBJ", val: 5n },
            { name: "FETCH_COLUMN", val: 7n },
            { name: "FETCH_CLASS", val: 8n },
            { name: "FETCH_KEY_PAIR", val: 12n },
            { name: "ATTR_ERRMODE", val: 3n },
            { name: "ATTR_DEFAULT_FETCH_MODE", val: 19n },
            { name: "ATTR_EMULATE_PREPARES", val: 20n },
            { name: "ATTR_DRIVER_NAME", val: 16n },
            { name: "ERRMODE_SILENT", val: 0n },
            { name: "ERRMODE_WARNING", val: 1n },
            { name: "ERRMODE_EXCEPTION", val: 2n },
            { name: "PARAM_NULL", val: 0n },
            { name: "PARAM_INT", val: 1n },
            { name: "PARAM_STR", val: 2n },
            { name: "PARAM_LOB", val: 3n },
            { name: "PARAM_BOOL", val: 5n },
        ],
        methods: [
            {
                name: "__construct", vis: 0,
                builtin: function (dsn) {
                    const dsnStr = rt.toStr(dsn || "sqlite::memory:");
                    const isMysql = /^mysql:/i.test(dsnStr);
                    let dbTarget = dsnStr;
                    if (isMysql) {
                        const m = /dbname=([^;\s]+)/i.exec(dsnStr);
                        dbTarget = m ? ("sqlite://" + m[1] + ".db") : "sqlite://default.db";
                    }
                    const opened = sqlCall("open", dbTarget, "1", "");
                    if (opened.error || !(opened.id >= 0)) {
                        rt.throwError("PDOException", opened.error || "failed to open database");
                    }
                    this.__connId = opened.id;
                    this.__mysqlCompat = isMysql;
                    this.__driver = isMysql ? "mysql" : "sqlite";
                    this.__errMode = 2n; // PHP 8 default: ERRMODE_EXCEPTION
                    this.__fetchMode = 4n;
                    this.__inTx = false;
                    this.__lastInsertId = 0;
                    this.__lastError = null;
                }
            },
            {
                name: "prepare", vis: 0,
                builtin: function (sql) {
                    const stmt = rt.allocInstance(rt.classes.get("pdostatement"));
                    stmt.__pdo = this;
                    stmt.__connId = this.__connId;
                    stmt.__sql = rt.toStr(sql);
                    stmt.__mysqlCompat = this.__mysqlCompat;
                    stmt.__errMode = this.__errMode;
                    stmt.__fetchMode = this.__fetchMode;
                    stmt.__columns = [];
                    stmt.__rows = [];
                    stmt.__cursor = 0;
                    stmt.__rowCount = 0;
                    stmt.props["queryString"] = stmt.__sql;
                    return stmt;
                }
            },
            {
                name: "query", vis: 0,
                builtin: function (sql, fetchMode) {
                    const stmt = rt.allocInstance(rt.classes.get("pdostatement"));
                    stmt.__pdo = this;
                    stmt.__connId = this.__connId;
                    stmt.__sql = rt.toStr(sql);
                    stmt.__mysqlCompat = this.__mysqlCompat;
                    stmt.__errMode = this.__errMode;
                    stmt.__fetchMode = (fetchMode !== undefined && rt.deref(fetchMode) !== null)
                        ? rt.toInt(fetchMode) : this.__fetchMode;
                    stmt.props["queryString"] = stmt.__sql;
                    const res = runSql(this.__connId, stmt.__sql, [], this.__mysqlCompat);
                    if (res.error) {
                        this.__lastError = res.error;
                        throwPdoError(res.error, this.__errMode);
                        return false;
                    }
                    stmt.__columns = res.columns || [];
                    stmt.__rows = res.rows || [];
                    stmt.__cursor = 0;
                    stmt.__rowCount = res.rows_affected || stmt.__rows.length || 0;
                    if (res.last_insert_rowid !== undefined && res.last_insert_rowid > 0) {
                        this.__lastInsertId = res.last_insert_rowid;
                    }
                    return stmt;
                }
            },
            {
                name: "exec", vis: 0,
                builtin: function (sql) {
                    const res = runSql(this.__connId, rt.toStr(sql), [], this.__mysqlCompat);
                    if (res.error) {
                        this.__lastError = res.error;
                        throwPdoError(res.error, this.__errMode);
                        return false;
                    }
                    if (res.last_insert_rowid !== undefined && res.last_insert_rowid > 0) {
                        this.__lastInsertId = res.last_insert_rowid;
                    }
                    return BigInt(res.rows_affected || 0);
                }
            },
            {
                name: "beginTransaction", vis: 0,
                builtin: function () {
                    const res = sqlCall("execute", String(this.__connId), "BEGIN TRANSACTION", "[]");
                    if (res.error) { throwPdoError(res.error, this.__errMode); return false; }
                    this.__inTx = true;
                    return true;
                }
            },
            {
                name: "commit", vis: 0,
                builtin: function () {
                    const res = sqlCall("execute", String(this.__connId), "COMMIT", "[]");
                    this.__inTx = false;
                    if (res.error) { throwPdoError(res.error, this.__errMode); return false; }
                    return true;
                }
            },
            {
                name: "rollBack", vis: 0,
                builtin: function () {
                    const res = sqlCall("execute", String(this.__connId), "ROLLBACK", "[]");
                    this.__inTx = false;
                    if (res.error) { throwPdoError(res.error, this.__errMode); return false; }
                    return true;
                }
            },
            { name: "inTransaction", vis: 0, builtin: function () { return !!this.__inTx; } },
            { name: "lastInsertId", vis: 0, builtin: function () { return String(this.__lastInsertId || 0); } },
            {
                name: "quote", vis: 0,
                builtin: function (s) { return "'" + rt.toStr(s).replace(/'/g, "''") + "'"; }
            },
            {
                name: "setAttribute", vis: 0,
                builtin: function (attr, val) {
                    const a = rt.toInt(attr);
                    if (a === 3n) this.__errMode = rt.toInt(val);
                    else if (a === 19n) this.__fetchMode = rt.toInt(val);
                    return true;
                }
            },
            {
                name: "getAttribute", vis: 0,
                builtin: function (attr) {
                    const a = rt.toInt(attr);
                    if (a === 3n) return this.__errMode;
                    if (a === 19n) return this.__fetchMode;
                    if (a === 16n) return this.__driver || "sqlite";
                    return null;
                }
            },
            { name: "errorCode", vis: 0, builtin: function () { return this.__lastError ? "HY000" : "00000"; } },
            {
                name: "errorInfo", vis: 0,
                builtin: function () {
                    const a = rt.makeArr();
                    rt.arrPushInPlace(a, this.__lastError ? "HY000" : "00000");
                    rt.arrPushInPlace(a, this.__lastError ? 1n : null);
                    rt.arrPushInPlace(a, this.__lastError || null);
                    return a;
                }
            },
        ]
    });

    // ---- SQLite3 / SQLite3Result / SQLite3Stmt ----
    R.defClass({
        name: "SQLite3Result", kind: "class",
        methods: [
            {
                name: "fetchArray", vis: 0,
                builtin: function (mode) {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return false;
                    const row = this.__rows[this.__cursor++];
                    const m = (mode !== undefined && rt.deref(mode) !== null) ? Number(rt.toInt(mode)) : 3;
                    const style = m === 1 ? "assoc" : m === 2 ? "num" : "both";
                    return buildRowByStyle(this.__columns || [], row, style);
                }
            },
            { name: "numColumns", vis: 0, builtin: function () { return BigInt((this.__columns || []).length); } },
            {
                name: "columnName", vis: 0,
                builtin: function (i) {
                    const idx = Number(rt.toInt(i));
                    return (this.__columns && idx >= 0 && idx < this.__columns.length) ? this.__columns[idx] : false;
                }
            },
            { name: "reset", vis: 0, builtin: function () { this.__cursor = 0; return true; } },
            { name: "finalize", vis: 0, builtin: function () { return true; } },
        ]
    });

    R.defClass({
        name: "SQLite3Stmt", kind: "class",
        methods: [
            {
                name: "bindValue", vis: 0,
                builtin: function (param, value) {
                    const k = rt.deref(param);
                    if (typeof k === "bigint" || typeof k === "number") {
                        if (!this.__posParams) this.__posParams = [];
                        this.__posParams[Number(k) - 1] = encodePhpParam(value);
                    } else {
                        if (!this.__namedParams) this.__namedParams = {};
                        this.__namedParams[rt.toStr(k)] = encodePhpParam(value);
                    }
                    return true;
                }
            },
            {
                name: "bindParam", vis: 0,
                builtin: function (param, value) {
                    const k = rt.deref(param);
                    if (typeof k === "bigint" || typeof k === "number") {
                        if (!this.__posParams) this.__posParams = [];
                        this.__posParams[Number(k) - 1] = encodePhpParam(value);
                    } else {
                        if (!this.__namedParams) this.__namedParams = {};
                        this.__namedParams[rt.toStr(k)] = encodePhpParam(value);
                    }
                    return true;
                }
            },
            {
                name: "execute", vis: 0,
                builtin: function () {
                    const params = (this.__posParams && this.__posParams.length > 0)
                        ? this.__posParams
                        : (this.__namedParams || []);
                    const res = runSql(this.__connId, this.__sql, params, false);
                    if (res.error) return false;
                    if (this.__db) {
                        this.__db.__changes = res.rows_affected || 0;
                        if (res.last_insert_rowid > 0) this.__db.__lastInsertId = res.last_insert_rowid;
                    }
                    const r = rt.allocInstance(rt.classes.get("sqlite3result"));
                    r.__columns = res.columns || [];
                    r.__rows = res.rows || [];
                    r.__cursor = 0;
                    return r;
                }
            },
            { name: "reset", vis: 0, builtin: function () { return true; } },
            { name: "clear", vis: 0, builtin: function () { this.__posParams = []; this.__namedParams = {}; return true; } },
            { name: "close", vis: 0, builtin: function () { return true; } },
        ]
    });

    R.defClass({
        name: "SQLite3", kind: "class",
        methods: [
            {
                name: "__construct", vis: 0,
                builtin: function (filename) {
                    const f = rt.toStr(filename || ":memory:");
                    const opened = sqlCall("open", f, "1", "");
                    if (opened.error || !(opened.id >= 0)) {
                        rt.throwError("Exception", opened.error || "Unable to open database");
                    }
                    this.__connId = opened.id;
                    this.__changes = 0;
                    this.__lastInsertId = 0;
                    this.__lastError = "";
                }
            },
            {
                name: "exec", vis: 0,
                builtin: function (sql) {
                    const res = runSql(this.__connId, rt.toStr(sql), [], false);
                    if (res.error) { this.__lastError = res.error; return false; }
                    this.__changes = res.rows_affected || 0;
                    if (res.last_insert_rowid > 0) this.__lastInsertId = res.last_insert_rowid;
                    return true;
                }
            },
            {
                name: "query", vis: 0,
                builtin: function (sql) {
                    const res = runSql(this.__connId, rt.toStr(sql), [], false);
                    if (res.error) { this.__lastError = res.error; return false; }
                    this.__changes = res.rows_affected || 0;
                    if (res.last_insert_rowid > 0) this.__lastInsertId = res.last_insert_rowid;
                    const r = rt.allocInstance(rt.classes.get("sqlite3result"));
                    r.__columns = res.columns || [];
                    r.__rows = res.rows || [];
                    r.__cursor = 0;
                    return r;
                }
            },
            {
                name: "querySingle", vis: 0,
                builtin: function (sql, entireRow) {
                    const res = runSql(this.__connId, rt.toStr(sql), [], false);
                    if (res.error) { this.__lastError = res.error; return false; }
                    const rows = res.rows || [];
                    if (rows.length === 0) return rt.truthy(entireRow) ? rt.makeArr() : null;
                    if (rt.truthy(entireRow)) return buildRowByStyle(res.columns || [], rows[0], "assoc");
                    return rows[0].length > 0 ? decodeSqlVal(rows[0][0]) : null;
                }
            },
            {
                name: "prepare", vis: 0,
                builtin: function (sql) {
                    const st = rt.allocInstance(rt.classes.get("sqlite3stmt"));
                    st.__db = this;
                    st.__connId = this.__connId;
                    st.__sql = rt.toStr(sql);
                    return st;
                }
            },
            { name: "changes", vis: 0, builtin: function () { return BigInt(this.__changes || 0); } },
            { name: "lastInsertRowID", vis: 0, builtin: function () { return BigInt(this.__lastInsertId || 0); } },
            { name: "lastErrorMsg", vis: 0, builtin: function () { return this.__lastError || "not an error"; } },
            { name: "lastErrorCode", vis: 0, builtin: function () { return this.__lastError ? 1n : 0n; } },
            { name: "busyTimeout", vis: 0, builtin: function () { return true; } },
            {
                name: "escapeString", vis: 0, isStatic: true,
                builtin: function (s) { return rt.toStr(s).replace(/'/g, "''"); }
            },
            {
                name: "close", vis: 0,
                builtin: function () {
                    if (this.__connId >= 0) {
                        sqlCall("close", String(this.__connId), "", "");
                        this.__connId = -1;
                    }
                    return true;
                }
            },
        ]
    });

    // ---- mysqli / mysqli_result / mysqli_stmt (backed by XDB SQLite + MySQL compat translator) ----
    let lastMysqliConnectError = "";
    let lastMysqliConnectErrno = 0n;
    let mysqliReportMode = 0n;

    function makeMysqliResult(columns, rows) {
        const res = rt.allocInstance(rt.classes.get("mysqli_result"));
        res.__columns = columns || [];
        res.__rows = rows || [];
        res.__cursor = 0;
        res.props["num_rows"] = BigInt(res.__rows.length);
        res.props["field_count"] = BigInt(res.__columns.length);
        return res;
    }

    R.defClass({
        name: "mysqli_result", kind: "class",
        props: [
            { name: "num_rows", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "field_count", vis: 0, hasDefault: true, defaultVal: 0n },
        ],
        methods: [
            {
                name: "fetch_assoc", vis: 0,
                builtin: function () {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return null;
                    return buildRowByStyle(this.__columns || [], this.__rows[this.__cursor++], "assoc");
                }
            },
            {
                name: "fetch_row", vis: 0,
                builtin: function () {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return null;
                    return buildRowByStyle(this.__columns || [], this.__rows[this.__cursor++], "num");
                }
            },
            {
                name: "fetch_array", vis: 0,
                builtin: function (mode) {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return null;
                    const m = (mode !== undefined && rt.deref(mode) !== null) ? Number(rt.toInt(mode)) : 3;
                    const style = m === 1 ? "assoc" : m === 2 ? "num" : "both";
                    return buildRowByStyle(this.__columns || [], this.__rows[this.__cursor++], style);
                }
            },
            {
                name: "fetch_object", vis: 0,
                builtin: function () {
                    if (!this.__rows || this.__cursor >= this.__rows.length) return null;
                    return buildRowByStyle(this.__columns || [], this.__rows[this.__cursor++], "obj");
                }
            },
            {
                name: "fetch_all", vis: 0,
                builtin: function (mode) {
                    const out = rt.makeArr();
                    const m = (mode !== undefined && rt.deref(mode) !== null) ? Number(rt.toInt(mode)) : 2;
                    const style = m === 1 ? "assoc" : m === 3 ? "both" : "num";
                    while (this.__rows && this.__cursor < this.__rows.length) {
                        rt.arrPushInPlace(out, buildRowByStyle(this.__columns || [], this.__rows[this.__cursor++], style));
                    }
                    return out;
                }
            },
            {
                name: "data_seek", vis: 0,
                builtin: function (offset) {
                    const o = Number(rt.toInt(offset));
                    if (o < 0 || o >= (this.__rows || []).length) return false;
                    this.__cursor = o;
                    return true;
                }
            },
            { name: "free", vis: 0, builtin: function () { this.__rows = []; return null; } },
            { name: "free_result", vis: 0, builtin: function () { this.__rows = []; return null; } },
            { name: "close", vis: 0, builtin: function () { this.__rows = []; return null; } },
        ]
    });

    R.defClass({
        name: "mysqli_stmt", kind: "class",
        props: [
            { name: "affected_rows", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "insert_id", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "num_rows", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "error", vis: 0, hasDefault: true, defaultVal: "" },
            { name: "errno", vis: 0, hasDefault: true, defaultVal: 0n },
        ],
        methods: [
            {
                name: "bind_param", vis: 0,
                builtin: function () {
                    this.__boundArgs = [];
                    for (let i = 1; i < arguments.length; i++) {
                        this.__boundArgs.push(arguments[i]);
                    }
                    return true;
                }
            },
            {
                name: "execute", vis: 0,
                builtin: function (paramsArr) {
                    const params = [];
                    const pa = rt.deref(paramsArr);
                    if (pa && pa.__php === 1) {
                        for (const v of pa.m.values()) params.push(encodePhpParam(v));
                    } else if (this.__boundArgs) {
                        for (let i = 0; i < this.__boundArgs.length; i++) params.push(encodePhpParam(this.__boundArgs[i]));
                    }
                    const res = runSql(this.__connId, this.__sql, params, true);
                    if (res.error) {
                        this.props["error"] = res.error;
                        this.props["errno"] = 1n;
                        if (this.__db) { this.__db.props["error"] = res.error; this.__db.props["errno"] = 1n; }
                        if ((mysqliReportMode & 2n) !== 0n) rt.throwError("mysqli_sql_exception", res.error);
                        return false;
                    }
                    this.__columns = res.columns || [];
                    this.__rows = res.rows || [];
                    this.__cursor = 0;
                    this.props["affected_rows"] = BigInt(res.rows_affected || 0);
                    this.props["insert_id"] = BigInt(res.last_insert_rowid || 0);
                    this.props["num_rows"] = BigInt(this.__rows.length);
                    if (this.__db) {
                        this.__db.props["affected_rows"] = this.props["affected_rows"];
                        if (res.last_insert_rowid > 0) this.__db.props["insert_id"] = this.props["insert_id"];
                    }
                    return true;
                }
            },
            {
                name: "get_result", vis: 0,
                builtin: function () {
                    return makeMysqliResult(this.__columns || [], this.__rows || []);
                }
            },
            { name: "store_result", vis: 0, builtin: function () { return true; } },
            { name: "close", vis: 0, builtin: function () { return true; } },
        ]
    });

    function mysqliConnectInto(self, host, user, pass, dbname) {
        const name = rt.toStr(dbname || "");
        const target = name
            ? ("sqlite://" + (/\.(?:db|sqlite|sqlite3)$/i.test(name) ? name : (name + ".db")))
            : "sqlite://default.db";
        const opened = sqlCall("open", target, "1", "");
        if (opened.error || !(opened.id >= 0)) {
            lastMysqliConnectError = opened.error || "failed to connect";
            lastMysqliConnectErrno = 2002n;
            self.__connId = -1;
            self.props["connect_error"] = lastMysqliConnectError;
            self.props["connect_errno"] = lastMysqliConnectErrno;
            if ((mysqliReportMode & 2n) !== 0n) rt.throwError("mysqli_sql_exception", lastMysqliConnectError);
            return false;
        }
        lastMysqliConnectError = "";
        lastMysqliConnectErrno = 0n;
        self.__connId = opened.id;
        self.props["connect_error"] = null;
        self.props["connect_errno"] = 0n;
        self.props["error"] = "";
        self.props["errno"] = 0n;
        return true;
    }

    function mysqliQueryImpl(self, sql) {
        const raw = rt.toStr(sql);
        const res = runSql(self.__connId, raw, [], true);
        if (res.error) {
            self.props["error"] = res.error;
            self.props["errno"] = 1n;
            if ((mysqliReportMode & 2n) !== 0n) rt.throwError("mysqli_sql_exception", res.error);
            return false;
        }
        self.props["error"] = "";
        self.props["errno"] = 0n;
        self.props["affected_rows"] = BigInt(res.rows_affected || 0);
        if (res.last_insert_rowid !== undefined && res.last_insert_rowid > 0) {
            self.props["insert_id"] = BigInt(res.last_insert_rowid);
        }
        if (Array.isArray(res.columns) && res.columns.length > 0) {
            return makeMysqliResult(res.columns, res.rows || []);
        }
        // SELECT / SHOW / DESCRIBE / PRAGMA with 0 columns or empty rows should still return mysqli_result
        const u = raw.trim().toUpperCase();
        if (u.startsWith("SELECT") || u.startsWith("SHOW") || u.startsWith("DESCRIBE") || u.startsWith("DESC ") || u.startsWith("PRAGMA") || u.startsWith("WITH")) {
            return makeMysqliResult(res.columns || [], res.rows || []);
        }
        return true;
    }

    R.defClass({
        name: "mysqli", kind: "class",
        props: [
            { name: "connect_error", vis: 0, hasDefault: true, defaultVal: null },
            { name: "connect_errno", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "error", vis: 0, hasDefault: true, defaultVal: "" },
            { name: "errno", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "affected_rows", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "insert_id", vis: 0, hasDefault: true, defaultVal: 0n },
            { name: "server_info", vis: 0, hasDefault: true, defaultVal: "8.0.36-zipp-xdb" },
            { name: "server_version", vis: 0, hasDefault: true, defaultVal: 80036n },
        ],
        methods: [
            {
                name: "__construct", vis: 0,
                builtin: function (host, user, pass, dbname) {
                    mysqliConnectInto(this, host, user, pass, dbname);
                }
            },
            {
                name: "real_connect", vis: 0,
                builtin: function (host, user, pass, dbname) {
                    return mysqliConnectInto(this, host, user, pass, dbname);
                }
            },
            { name: "set_charset", vis: 0, builtin: function () { return true; } },
            { name: "select_db", vis: 0, builtin: function () { return true; } },
            { name: "options", vis: 0, builtin: function () { return true; } },
            {
                name: "query", vis: 0,
                builtin: function (sql) { return mysqliQueryImpl(this, sql); }
            },
            {
                name: "multi_query", vis: 0,
                builtin: function (sql) {
                    const res = mysqliQueryImpl(this, sql);
                    return res !== false;
                }
            },
            {
                name: "prepare", vis: 0,
                builtin: function (sql) {
                    const st = rt.allocInstance(rt.classes.get("mysqli_stmt"));
                    st.__db = this;
                    st.__connId = this.__connId;
                    st.__sql = rt.toStr(sql);
                    return st;
                }
            },
            {
                name: "real_escape_string", vis: 0,
                builtin: function (s) {
                    return rt.toStr(s)
                        .replace(/\\/g, "\\\\")
                        .replace(/'/g, "\\'")
                        .replace(/\0/g, "\\0")
                        .replace(/\n/g, "\\n")
                        .replace(/\r/g, "\\r");
                }
            },
            {
                name: "escape_string", vis: 0,
                builtin: function (s) {
                    return rt.toStr(s)
                        .replace(/\\/g, "\\\\")
                        .replace(/'/g, "\\'")
                        .replace(/\0/g, "\\0")
                        .replace(/\n/g, "\\n")
                        .replace(/\r/g, "\\r");
                }
            },
            {
                name: "begin_transaction", vis: 0,
                builtin: function () { return !sqlCall("execute", String(this.__connId), "BEGIN TRANSACTION", "[]").error; }
            },
            {
                name: "commit", vis: 0,
                builtin: function () { return !sqlCall("execute", String(this.__connId), "COMMIT", "[]").error; }
            },
            {
                name: "rollback", vis: 0,
                builtin: function () { return !sqlCall("execute", String(this.__connId), "ROLLBACK", "[]").error; }
            },
            { name: "autocommit", vis: 0, builtin: function () { return true; } },
            { name: "more_results", vis: 0, builtin: function () { return false; } },
            { name: "next_result", vis: 0, builtin: function () { return false; } },
            { name: "ping", vis: 0, builtin: function () { return this.__connId >= 0; } },
            {
                name: "close", vis: 0,
                builtin: function () {
                    if (this.__connId >= 0) {
                        sqlCall("close", String(this.__connId), "", "");
                        this.__connId = -1;
                    }
                    return true;
                }
            },
        ]
    });

    // ---- Procedural mysqli_* functions ----
    reg("mysqli_report", function (flags) { mysqliReportMode = rt.toInt(flags); return true; });
    reg("mysqli_connect", function (h, u, p, db) {
        const obj = rt.allocInstance(rt.classes.get("mysqli"));
        return mysqliConnectInto(obj, h, u, p, db) ? obj : false;
    });
    reg("mysqli_init", function () { return rt.allocInstance(rt.classes.get("mysqli")); });
    reg("mysqli_connect_error", function () { return lastMysqliConnectError || null; });
    reg("mysqli_connect_errno", function () { return lastMysqliConnectErrno; });
    reg("mysqli_error", function (db) { const d = rt.deref(db); return (d && d.props && d.props["error"]) || ""; });
    reg("mysqli_errno", function (db) { const d = rt.deref(db); return (d && d.props && d.props["errno"]) || 0n; });
    reg("mysqli_set_charset", function () { return true; });
    reg("mysqli_select_db", function () { return true; });
    reg("mysqli_query", function (db, sql) { return mysqliQueryImpl(rt.deref(db), sql); });
    reg("mysqli_real_escape_string", function (db, s) {
        return rt.toStr(s).replace(/\\/g, "\\\\").replace(/'/g, "\\'").replace(/\0/g, "\\0").replace(/\n/g, "\\n").replace(/\r/g, "\\r");
    });
    reg("mysqli_escape_string", function (db, s) {
        return rt.toStr(s).replace(/\\/g, "\\\\").replace(/'/g, "\\'").replace(/\0/g, "\\0").replace(/\n/g, "\\n").replace(/\r/g, "\\r");
    });
    reg("mysqli_fetch_assoc", function (res) {
        const r = rt.deref(res);
        if (!r || !r.__rows || r.__cursor >= r.__rows.length) return null;
        return buildRowByStyle(r.__columns || [], r.__rows[r.__cursor++], "assoc");
    });
    reg("mysqli_fetch_row", function (res) {
        const r = rt.deref(res);
        if (!r || !r.__rows || r.__cursor >= r.__rows.length) return null;
        return buildRowByStyle(r.__columns || [], r.__rows[r.__cursor++], "num");
    });
    reg("mysqli_fetch_array", function (res, mode) {
        const r = rt.deref(res);
        if (!r || !r.__rows || r.__cursor >= r.__rows.length) return null;
        const m = (mode !== undefined && rt.deref(mode) !== null) ? Number(rt.toInt(mode)) : 3;
        const style = m === 1 ? "assoc" : m === 2 ? "num" : "both";
        return buildRowByStyle(r.__columns || [], r.__rows[r.__cursor++], style);
    });
    reg("mysqli_fetch_object", function (res) {
        const r = rt.deref(res);
        if (!r || !r.__rows || r.__cursor >= r.__rows.length) return null;
        return buildRowByStyle(r.__columns || [], r.__rows[r.__cursor++], "obj");
    });
    reg("mysqli_num_rows", function (res) { const r = rt.deref(res); return (r && r.props && r.props["num_rows"]) || 0n; });
    reg("mysqli_affected_rows", function (db) { const d = rt.deref(db); return (d && d.props && d.props["affected_rows"]) || 0n; });
    reg("mysqli_insert_id", function (db) { const d = rt.deref(db); return (d && d.props && d.props["insert_id"]) || 0n; });
    reg("mysqli_free_result", function (res) { const r = rt.deref(res); if (r) r.__rows = []; return null; });
    reg("mysqli_close", function (db) {
        const d = rt.deref(db);
        if (d && d.__connId >= 0) { sqlCall("close", String(d.__connId), "", ""); d.__connId = -1; }
        return true;
    });
    reg("mysqli_prepare", function (db, sql) {
        const d = rt.deref(db);
        const st = rt.allocInstance(rt.classes.get("mysqli_stmt"));
        st.__db = d;
        st.__connId = d.__connId;
        st.__sql = rt.toStr(sql);
        return st;
    });
    reg("mysqli_stmt_bind_param", function (st) {
        const s = rt.deref(st);
        s.__boundArgs = [];
        for (let i = 2; i < arguments.length; i++) s.__boundArgs.push(arguments[i]);
        return true;
    });
    reg("mysqli_stmt_execute", function (st, params) {
        const s = rt.deref(st);
        return R.callMethod(s, "execute", params !== undefined ? [params] : []);
    });
    reg("mysqli_stmt_get_result", function (st) {
        const s = rt.deref(st);
        return makeMysqliResult(s.__columns || [], s.__rows || []);
    });
    reg("mysqli_stmt_close", function () { return true; });
    reg("mysqli_begin_transaction", function (db) {
        const d = rt.deref(db);
        return !sqlCall("execute", String(d.__connId), "BEGIN TRANSACTION", "[]").error;
    });
    reg("mysqli_commit", function (db) {
        const d = rt.deref(db);
        return !sqlCall("execute", String(d.__connId), "COMMIT", "[]").error;
    });
    reg("mysqli_rollback", function (db) {
        const d = rt.deref(db);
        return !sqlCall("execute", String(d.__connId), "ROLLBACK", "[]").error;
    });
})(__zipp_php);
