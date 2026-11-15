//- /src/module-info.java
module r2.app {
    requires java.sql;
    requires transitive java.logging;
    requires /*@missing*/java.missing;
    exports exported;
    exports exported.sub to java.base;
    exports /*@nopkg*/missing.pkg;
    opens exported;
    opens /*@absent*/absent.pkg;
}
//- /src/exported/Api.java
package exported;

public class Api {}
//- /src/exported/sub/Impl.java
package exported.sub;

public class Impl {}
//? diag unresolved-module missing
//? diag unresolved-module nopkg
//? diag unresolved-module absent
//? diag exact
