import type { ValidationTargets } from "hono";
import { Hono } from "hono";
import { z, type ZodSchema } from "zod";
import { HTTPException } from "hono/http-exception";
import { drizzle } from "drizzle-orm/node-postgres";
import { greetingsTable } from "./db/schema";
import { eq } from "drizzle-orm";
import { zValidator as zv } from "@hono/zod-validator";
import { Client } from "pg";

const databaseUrl = process.env.DATABASE_URL;

if (!databaseUrl) {
	throw new Error("DATABASE_URL is not set");
}

const client = new Client({ connectionString: databaseUrl });
await client.connect();

const db = drizzle(client);

const app = new Hono();

const greetingSchema = z.object({
	greeting: z.string().max(100, "Greeting must be 100 characters or less"),
});
const greetingParamSchema = z.object({
	id: z.coerce.number().int().positive(),
});
const greetingQuerySchema = z.object({
	offset: z.coerce.number().int().min(0).max(100).default(0),
	limit: z.coerce.number().int().min(1).max(100).default(10),
});

export const zValidator = <
	T extends ZodSchema,
	Target extends keyof ValidationTargets,
>(
	target: Target,
	schema: T,
) =>
	zv(target, schema, (result, c) => {
		if (!result.success) {
			const error = result.error.issues[0];
			const res = c.json({
				error: { message: error.message, path: error.path },
			});
			throw new HTTPException(400, {
				cause: result.error,
				message: error.message,
				res,
			});
		}
	});

app.onError((error, c) => {
	console.error(error);
	if (error instanceof HTTPException) {
		if (error.res) {
			return error.getResponse();
		} else {
			return c.json({ error: { message: error.message } }, error.status);
		}
	}
	return c.json({ error: { message: error.message } }, 500);
});

app
	.get("/", zValidator("query", greetingQuerySchema), async (c) => {
		const { offset, limit } = c.req.valid("query");
		const items = await (async () => {
			try {
				return await db
					.select()
					.from(greetingsTable)
					.offset(offset)
					.limit(limit);
			} catch (error) {
				console.log(error);
				throw new HTTPException(500, { cause: error });
			}
		})();
		const totalCount = await (async () => {
			try {
				return await db.$count(greetingsTable);
			} catch (error) {
				throw new HTTPException(500, { cause: error });
			}
		})();
		return c.json({ items, totalCount });
	})
	.get("/:id", zValidator("param", greetingParamSchema), async (c) => {
		const { id } = c.req.valid("param");
		const greetings = await (async () => {
			try {
				return await db
					.select()
					.from(greetingsTable)
					.where(eq(greetingsTable.id, id))
					.limit(1);
			} catch (error) {
				throw new HTTPException(500, { cause: error });
			}
		})();
		if (greetings.length === 0) {
			throw new HTTPException(404);
		}
		return c.json(greetings[0]);
	})
	.post("/", zValidator("json", greetingSchema), async (c) => {
		const greeting = c.req.valid("json");
		const inserted = await (async () => {
			try {
				return await db.insert(greetingsTable).values(greeting).returning();
			} catch (error) {
				throw new HTTPException(500, { cause: error });
			}
		})();
		return c.json(inserted);
	});

export default {
	port: process.env.PORT ?? 3000,
	fetch: app.fetch,
};
