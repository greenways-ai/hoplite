import type { APIRoute } from "astro";
import { stackFootprints } from "../../lib/benchmark-data";

export const prerender = true;

export const GET: APIRoute = () =>
  new Response(`${JSON.stringify(stackFootprints, null, 2)}\n`, {
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "public, max-age=300",
    },
  });
