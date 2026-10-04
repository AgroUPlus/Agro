// Prints SQL that adds N active accounts, one device token each, and a friend graph, to a
// migrated Agro database. Tokens go to tokens.json for run.mjs.
//
//   node seed.mjs 5000 10 > seed.sql && sqlite3 agro_data.db < seed.sql
//
// Load testing only: never point this at a database anyone uses.
import { createHash, randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";

const users = Number(process.argv[2] ?? 5000);
const friendsEach = Number(process.argv[3] ?? 10);
const now = new Date().toISOString();
const tokens = [];
const out = ["BEGIN;"];

for (let i = 0; i < users; i++) {
  const name = `load${i}`;
  const secret = randomBytes(32).toString("base64url");
  const hash = createHash("sha256").update(secret).digest("hex");
  tokens.push({ username: name, token: secret, device: `wanda-load-${i}` });
  out.push(
    `INSERT INTO users (id, username, api_key, created_at, role, state, show_now_playing)
     VALUES ('id-${i}', '${name}', '', '${now}', 'member', 'active', 1);`,
    `INSERT INTO app_passwords (token, user_id, label, created_at, token_prefix, token_hash)
     VALUES ('${hash}', 'id-${i}', 'load', '${now}', '${secret.slice(0, 8)}', '${hash}');`,
  );
  for (let f = 1; f <= friendsEach / 2; f++) {
    const other = `load${(i + f) % users}`;
    out.push(
      `INSERT OR IGNORE INTO friendships (user_id, friend_id, state, created_at)
       VALUES ('${name}', '${other}', 'accepted', '${now}');`,
    );
  }
}
out.push("COMMIT;");
writeFileSync(new URL("tokens.json", import.meta.url), JSON.stringify(tokens));
console.log(out.join("\n"));
