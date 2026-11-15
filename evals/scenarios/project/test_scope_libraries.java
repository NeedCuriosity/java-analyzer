//- /org/testing/annotation/Commit.java lib:test-support scope:test
package org.testing.annotation;

public @interface Commit {}
//- /org/testing/CommitAssertions.java lib:test-support scope:test
package org.testing;

public class CommitAssertions {
    public static void assertCommitted() {}
}
//- /org/shared/CommonResult.java lib:shared
package org.shared;

public class CommonResult {
    public static CommonResult success() { return new CommonResult(); }
}
//- /app/Main.java
package app;

class Main {
    void run() {
        Comm/*@main*/;
    }
}
//- /app/MainTest.java root:test
package app;

import org.testing.CommitAssertions;

class MainTest {
    void run() {
        CommitAssertions.assertCommitted();
        Comm/*@test*/;
    }
}
//? complete main has CommonResult
//? complete main lacks Commit CommitAssertions
//? complete test has CommonResult Commit CommitAssertions
//? xfail test scope 的依赖没有和主源码根隔离
