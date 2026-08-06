from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BODY = b"Hello from Hoplite\n"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        if self.path != "/hello":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("content-type", "text/plain")
        self.send_header("x-hoplite", "true")
        self.send_header("content-length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)

    def log_message(self, _format: str, *_args: object) -> None:
        return


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
