//- /app/src/main/java/module-info.java
module m.app {
    requires java.logging;
    requires m.core;
}
//- /app/src/main/java/app/Main.java
package app;

import java.util.List;
import java.util.logging.Logger;
import java.util.concurrent.*;
import static java.util.Objects.requireNonNull;
import /*@sql*/java.sql.Connection;
import /*@sql_all*/java.sql.*;
import static /*@sql_static*/java.sql.Types.INTEGER;
import /*@internal*/jdk.internal.misc.Unsafe;
import core.api.Api;
import /*@core_internal*/core.internal.Secret;
import /*@other*/other.api.Tool;

public class Main {
    List<Logger> loggers;
}
//- /core/src/main/java/module-info.java module:core
module m.core {
    exports core.api;
}
//- /core/src/main/java/core/api/Api.java module:core
package core.api;

public class Api {}
//- /core/src/main/java/core/internal/Secret.java module:core
package core.internal;

public class Secret {}
//- /other/src/main/java/module-info.java module:other
module m.other {
    exports other.api;
}
//- /other/src/main/java/other/api/Tool.java module:other
package other.api;

public class Tool {}
//? module app core other
//? module core
//? module other
//? diag inaccessible-member sql
//? diag inaccessible-member sql_all
//? diag inaccessible-member sql_static
//? diag inaccessible-member internal
//? diag inaccessible-member core_internal
//? diag inaccessible-member other
//? diag exact
