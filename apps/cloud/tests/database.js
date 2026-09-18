import { readFileSync, readdirSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";

export function database() {
  const sql = new DatabaseSync(":memory:");
  const directory = new URL("../migrations/", import.meta.url);
  for (const name of readdirSync(directory)
    .filter((name) => name.endsWith(".sql"))
    .sort()) {
    sql.exec(readFileSync(new URL(name, directory), "utf8"));
  }
  const db = {
    exec: async (query) => sql.exec(query),
    prepare(query) {
      const statement = (args = []) => {
        const execute = () => {
          const prepared = sql.prepare(query);
          const results = prepared.all(...args);
          const changes = sql
            .prepare(
              "SELECT changes() AS changes, last_insert_rowid() AS last_row_id",
            )
            .get();
          return { results, success: true, meta: changes };
        };
        return {
          bind: (...values) => statement(values),
          first: async (column) => {
            const row = execute().results[0];
            return column ? (row?.[column] ?? null) : (row ?? null);
          },
          all: async () => execute(),
          run: async () => execute(),
          execute,
        };
      };
      return statement();
    },
    async batch(statements) {
      sql.exec("BEGIN IMMEDIATE");
      try {
        const result = statements.map((statement) => statement.execute());
        sql.exec("COMMIT");
        return result;
      } catch (error) {
        sql.exec("ROLLBACK");
        throw error;
      }
    },
  };
  return { db, sql };
}
