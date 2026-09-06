//! The `PgBouncer` messages worth a row.
//!
//! Every entry was read out of the `PgBouncer` sources, with the file it is
//! written from. A line is recognized when its message, or the reason inside
//! its `closing because:`, starts with one of these. Everything else is
//! dropped: on a default install the log also carries a line per connection
//! opened and closed, which is traffic, not an event.

/// The messages a `pgbouncer_events` row is written for.
pub const RECOGNIZED: &[&str] = &[
    // The database is not giving out connections.
    "cannot connect",                // objects.c:1722
    "connect failed",                // server.c:839
    "server conn crashed?",          // server.c:798
    "server DNS lookup failed",      // objects.c:1648
    "server login failed",           // server.c:106, with `<LEVEL> <text>`
    "server login has been failing", // objects.c:881, with the cached error
    // The pooler is turning connections away or evicting live ones.
    "evicted",                                                   // objects.c:1804
    "bouncer resources exhaustion",                              // client.c:593
    "out of memory",                                             // server.c:651
    "no memory for pool",                                        // client.c:351
    "no memory for authentication pool",                         // client.c:179
    "too many servers in the pool",                              // janitor.c:572
    "no more connections allowed (max_client_conn)",             // client.c:1086
    "client connections exceeded (max_db_client_connections)",   // client.c:481
    "client connections exceeded (max_user_client_connections)", // client.c:517
    // The queue and the timeouts that empty it.
    "query_wait_timeout",       // janitor.c:464
    "query_timeout",            // janitor.c:462, the client side
    "query timeout",            // janitor.c:667, the server side
    "idle transaction timeout", // janitor.c:670
    "transaction timeout",      // janitor.c:674
    "cancel_wait_timeout",      // janitor.c:477
    "connect timeout",          // janitor.c:689
    "client_login_timeout",     // janitor.c:489 and :741
    "suspend_timeout",          // janitor.c:70
    // The server connection cannot be handed out again.
    "idle server got dirty",        // janitor.c:147
    "SV_IDLE server got dirty",     // janitor.c:533
    "SV_USED server got dirty",     // janitor.c:535
    "reset query failed",           // objects.c:1174
    "test query failed",            // janitor.c:168
    "exec_on_connect query failed", // server.c:223
    "var change failed",            // objects.c:957
    "invalid server parameter",     // server.c:450
    // The pooler process itself.
    "pooler is shutting down",             // objects.c:2051
    "client connections dropped, exiting", // janitor.c:896
    "server connections dropped, exiting", // janitor.c:888
    "accept() failed",                     // pooler.c:388
    "cannot listen on",                    // pooler.c:226
    "kernel file descriptor limit",        // main.c:858
    "process up",                          // main.c:1178
    "TLS configuration could not be reloaded, keeping old configuration", // main.c:644
    "RELOAD Failed, see logs for more details", // admin.c:1152
    // Authentication and protocol.
    "password authentication failed",    // client.c:1192
    "SASL authentication failed",        // client.c:1349
    "LDAP authentication failed",        // ldapauth.c:582
    "PAM authentication failed",         // pam.c:287
    "certificate authentication failed", // client.c:273
    "no such user",                      // client.c:664
    "no such database",                  // objects.c:2041
    "broken auth file",                  // loader.c:749
    "error response from auth_query",    // client.c:810
    "unable to send auth_query",         // client.c:218
    "bad packet",                        // admin.c:1785
    "bad pkt header",                    // server.c:815
    "failed to parse packet",            // client.c:1577
    "old V2 protocol not supported",     // client.c:1299
    "TLS handshake error",               // sbuf.c:1404
];
