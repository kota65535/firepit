import {zValidator} from "@hono/zod-validator";
import {Hono} from "hono";
import {createClient} from "redis";
import {z} from "zod";
import {HTTPException} from "hono/http-exception";

const app = new Hono();

const client = await createClient()
	.on("error", (err) => console.log("Redis Client Error", err))
	.connect();

const greetingSchema = z.object({
	greeting: z.string().max(100, "Greeting must be 100 characters or less"),
});

const KEY = "greeting";

app
	.get("/", async (c) => {
		let greeting: string | null;
		try {
			greeting = await client.get(KEY);
		} catch (error) {
			throw new HTTPException(500, { cause: error });
		}
		return c.json({ greeting });
	})
	.post("/", zValidator("json", greetingSchema), async (c) => {
		const body = c.req.valid("json");
		try {
			await client.set(KEY, body.greeting);
		} catch (error) {
			throw new HTTPException(500, { cause: error });
		}
		return c.json(body);
	});

export default {
	port: process.env.PORT ?? 3000,
	fetch: app.fetch,
};
