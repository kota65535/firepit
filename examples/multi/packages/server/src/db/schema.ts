import { integer, pgTable, varchar } from "drizzle-orm/pg-core";

export const greetingsTable = pgTable("greetings", {
	id: integer().primaryKey().generatedAlwaysAsIdentity(),
	greeting: varchar({ length: 255 }).notNull(),
});
