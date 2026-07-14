const fs = require("fs");
let t = fs.readFileSync("crates/modeld-proxy/src/config.rs", "utf8");
t = t.replace('assert_eq!(cfg.bind_address, "0.0.0.0");', 'assert_eq!(cfg.bind_address, "127.0.0.1");');
t = t.replace('assert_eq!(cfg.bind_address, "0.0.0.0"); // default', 'assert_eq!(cfg.bind_address, "127.0.0.1"); // default');
fs.writeFileSync("crates/modeld-proxy/src/config.rs", t, "utf8");
console.log("config.rs default binding updated");
