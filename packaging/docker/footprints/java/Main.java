import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.Executors;

public final class Main {
  private static final byte[] BODY = "Hello from Hoplite\n".getBytes(StandardCharsets.UTF_8);

  private static void hello(HttpExchange exchange) throws IOException {
    if (!"/hello".equals(exchange.getRequestURI().getPath())) {
      exchange.sendResponseHeaders(404, -1);
      exchange.close();
      return;
    }
    exchange.getResponseHeaders().set("content-type", "text/plain");
    exchange.getResponseHeaders().set("x-hoplite", "true");
    exchange.sendResponseHeaders(200, BODY.length);
    exchange.getResponseBody().write(BODY);
    exchange.close();
  }

  public static void main(String[] args) throws Exception {
    HttpServer server = HttpServer.create(new InetSocketAddress("0.0.0.0", 8080), 1024);
    server.createContext("/hello", Main::hello);
    server.setExecutor(Executors.newFixedThreadPool(4));
    server.start();
  }
}
