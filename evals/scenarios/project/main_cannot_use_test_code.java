//- /org/testing/Checks.java lib:test-support scope:test
package org.testing;

public class Checks {
    public static void check(boolean condition) {}
}
//- /app/Service.java
package app;

import /*@lib*/org.testing.Checks;

public class Service {
    int value() { return 1; }

    void misuse() {
        /*@helper*/Fixtures fixtures = null;
        /*@use*/Checks.check(true);
    }
}
//- /app/Fixtures.java root:test
package app;

public class Fixtures {
    static Service service() { return new Service(); }
}
//- /app/ServiceTest.java root:test
package app;

import org.testing.Checks;

class ServiceTest {
    void run() {
        Checks.check(Fixtures.service().value() == 1);
    }
}
//? diag unresolved-type helper
//? diag unresolved-import lib
//? diag unresolved-name use
//? diag exact
//? xfail 主源码根能看到测试源码根里的类和 test scope 的依赖
