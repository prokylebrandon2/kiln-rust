/* kiln_rs: Rust engine for Kiln (.kln "KLNA" archives). JSON in, JSON out. */
#ifndef KILN_RS_H
#define KILN_RS_H
#ifdef __cplusplus
extern "C" {
#endif

/* Progress / cancel callback. `event` is a JSON object {"frac":0..1 or -1,"done":N,"total":N,"file":"..."},
 * or NULL when the engine only wants to know whether to stop. Return non-zero to cancel. */
typedef int (*kiln_rs_cb)(const char *event, void *user);

/* op = "version" : {}                                   -> {"kiln":"rs-0.1.0","helper":""}
 * op = "open"    : {"path","password"}                  -> {"ok":true,"kind":"archive","encrypted":b,
 *                                                          "entries":[{"p","s","m"}],"packed":N}
 *                                                        or {"ok":false,"error"|"need_password",...}
 * op = "test"    : {"path","password"}                  -> {"ok","message","done","bad_password"}
 * op = "extract" : {"path","password","dest","overwrite":bool,
 *                   "members":[{"p":"dir/file","dir":false},...]}   ([] = everything)
 *                                                        -> {"ok","message","done","bad_password"}
 *                                                           or {"ok":false,"cancelled":true,"message":"Cancelled"}
 * op = "pack"    : {"items":[paths],"dest","encrypt":b,"store":b,"overwrite":b,"password","each":b,"jobs":N}
 *                   each=false: dest is the archive; each=true: dest is the OUTPUT FOLDER, one archive per file
 *                   (plain: mirrored layout, NAME.kln; encrypted: random names + an encrypted _index.kln).
 *                   jobs 0 = automatic                   -> {"ok","message","done","packed":[paths],"warnings":[..]}
 *                                                           or {"ok":false,"cancelled":true,"message":"Cancelled"}
 *                                                           or {"unsupported":true} (caller falls back to Python)
 * Result: NUL-terminated UTF-8 JSON owned by the engine; release with kiln_rs_free. Never NULL. */
char *kiln_rs_call(const char *op, const char *args_json, kiln_rs_cb cb, void *user);
void kiln_rs_free(char *s);

#ifdef __cplusplus
}
#endif
#endif
