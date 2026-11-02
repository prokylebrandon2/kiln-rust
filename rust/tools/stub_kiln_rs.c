/* A stand-in for libkiln_rs.so that speaks the kiln_rs.h protocol, used to test the app-side loader
 * (kiln_rs_engine.py, kiln_scripts.install_engine, kiln_app dispatch) without a Rust toolchain.
 * gcc -shared -fPIC -o stub.so tools/stub_kiln_rs.c */
#include <stdlib.h>
#include <string.h>
typedef int (*cb_t)(const char *, void *);
static char *dup(const char *s) { char *p = malloc(strlen(s) + 1); strcpy(p, s); return p; }
char *kiln_rs_call(const char *op, const char *args, cb_t cb, void *user) {
    if (!strcmp(op, "version")) return dup("{\"kiln\":\"rs-9.9.9\",\"helper\":\"\"}");
    if (args && strstr(args, "unsupported")) return dup("{\"ok\":false,\"unsupported\":true,\"message\":\"no\"}");
    if (args && strstr(args, "crash")) return NULL;
    if (!strcmp(op, "open")) return dup("{\"ok\":true,\"kind\":\"archive\",\"encrypted\":false,\"entries\":[{\"p\":\"\xc3\xa9.txt\",\"s\":1,\"m\":2}],\"packed\":3,\"via\":\"stub\"}");
    if (!strcmp(op, "extract") || !strcmp(op, "test")) {
        int i;
        for (i = 0; i < 5; i++) {
            if (cb) {
                if (cb(NULL, user)) return dup("{\"ok\":false,\"cancelled\":true,\"message\":\"Cancelled\"}");
                if (cb("{\"frac\":0.5,\"done\":1,\"total\":0,\"file\":\"x\"}", user))
                    return dup("{\"ok\":false,\"cancelled\":true,\"message\":\"Cancelled\"}");
            }
        }
        return dup("{\"ok\":true,\"message\":\"done by stub\",\"done\":5,\"bad_password\":false}");
    }
    return dup("{\"ok\":false,\"error\":\"unknown\"}");
}
void kiln_rs_free(char *p) { free(p); }
