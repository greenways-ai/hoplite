from pathlib import Path

MODULE = Path("core/nginx/ngx_http_hoplite_module.c")
SELF = Path(__file__)

source = MODULE.read_text()

old = '''        if (source->final_read) {
            ngx_http_hoplite_source_complete(ctx, rc);
            return;
        }
        if (rc == NGX_AGAIN || ngx_http_hoplite_source_pending(ctx)) {
            if (ngx_http_hoplite_source_wait(ctx) == NGX_ERROR) {
                ngx_http_hoplite_source_fail(
                    ctx, NGX_ERROR,
                    "hoplite response source could not wait for output");
            }
            return;
        }
'''
new = '''        if (rc == NGX_AGAIN || ngx_http_hoplite_source_pending(ctx)) {
            if (ngx_http_hoplite_source_wait(ctx) == NGX_ERROR) {
                ngx_http_hoplite_source_fail(
                    ctx, NGX_ERROR,
                    "hoplite response source could not wait for output");
            }
            return;
        }
        if (source->final_read) {
            ngx_http_hoplite_source_complete(ctx, rc);
            return;
        }
'''
if source.count(old) != 1:
    raise SystemExit("expected one final source pump block")
source = source.replace(old, new)

old = '''    if (write->timer_set) {
        ngx_del_timer(write);
    }
    ngx_http_hoplite_source_pump(ctx);
}
'''
new = '''    if (write->timer_set) {
        ngx_del_timer(write);
    }
    if (source->final_read) {
        ngx_http_hoplite_source_complete(ctx, rc);
        return;
    }
    ngx_http_hoplite_source_pump(ctx);
}
'''
if source.count(old) != 1:
    raise SystemExit("expected one response source write completion block")
source = source.replace(old, new)

MODULE.write_text(source)
SELF.unlink()
