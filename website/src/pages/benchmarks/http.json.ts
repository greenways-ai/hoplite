import type { APIRoute } from "astro";
import { httpBenchmark } from "../../lib/benchmark-data";

export const prerender = true;

export const GET: APIRoute = () =>
  new Response(`${JSON.stringify(httpBenchmark, null, 2)}\n`, {
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "public, max-age=300",
    },
  });
