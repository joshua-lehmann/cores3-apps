/**
 * A Cloudflare Worker that serves the built page (`npm run build`).
 *
 * It cannot decode backtraces. Like the other servers, it answers a GET for a
 * path without a file with `index.html`, so that routes inside the page work.
 * Unlike the Rust server, it does this also for missing files with an
 * extension, and for `/api/backtrace`.
 */

/** The Cloudflare binding that serves the static files of the build. */
type AssetsBinding = {
  fetch(request: Request): Promise<Response>;
};

/** The bindings that Cloudflare passes to the Worker. */
type WorkerEnvironment = {
  ASSETS: AssetsBinding;
};

export default {
  async fetch(request: Request, environment: WorkerEnvironment): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname.startsWith("/api/firmware/")) {
      const match = /^\/api\/firmware\/(v\d+\.\d+\.\d+)\/([a-z][a-z0-9_]*\.bin)$/.exec(url.pathname);
      if (request.method !== "GET" || !match || match[1].length > 32 || match[2].length > 100) return new Response("Invalid asset", { status: 400 });
      const upstream = await fetch(`https://github.com/joshua-lehmann/cores3-apps/releases/download/${match[1]}/${match[2]}`);
      if (!upstream.ok) return new Response("Could not download release asset", { status: 502 });
      return new Response(upstream.body, { headers: { "Content-Type": "application/octet-stream", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" } });
    }
    const response = await environment.ASSETS.fetch(request);
    if (response.status !== 404 || request.method !== "GET") return response;

    const fallbackUrl = new URL(request.url);
    fallbackUrl.pathname = "/index.html";
    return environment.ASSETS.fetch(new Request(fallbackUrl, request));
  },
};
