//? module app core api@provided lib:servlet@provided lib:driver@runtime lib:junit@test testkit/tests@test
//? module core
//? module api
//? module testkit
//- /javax/servlet/Servlet.java lib:servlet
package javax.servlet;

public interface Servlet {}
//- /org/driver/Driver.java lib:driver
package org.driver;

public class Driver {}
//- /org/junit/Assert.java lib:junit
package org.junit;

public class Assert {
    public static void assertTrue(boolean condition) {}
}
//- /core/src/main/java/demo/core/Repo.java module:core
package demo.core;

public class Repo {}
//- /core/src/test/java/demo/core/RepoFixtures.java module:core root:test
package demo.core;

public class RepoFixtures {}
//- /api/src/main/java/demo/api/Endpoint.java module:api
package demo.api;

public interface Endpoint {}
//- /testkit/src/main/java/demo/testkit/Kit.java module:testkit
package demo.testkit;

public class Kit {}
//- /testkit/src/test/java/demo/testkit/KitFixtures.java module:testkit root:test
package demo.testkit;

public class KitFixtures {}
//- /app/src/main/java/demo/app/Service.java module:app
package demo.app;

import demo.api.Endpoint;
import demo.core.Repo;
import javax.servlet.Servlet;
import /*@driver*/org.driver.Driver;
import /*@junit*/org.junit.Assert;
import /*@fixtures*/demo.core.RepoFixtures;
import /*@kit*/demo.testkit.KitFixtures;

public class Service implements Endpoint, Servlet {
    Repo repo;
}
//- /app/src/test/java/demo/app/ServiceTest.java module:app root:test
package demo.app;

import demo.core.Repo;
import demo.testkit.KitFixtures;
import org.driver.Driver;
import org.junit.Assert;
import /*@testfixtures*/demo.core.RepoFixtures;

class ServiceTest {
    Repo repo;
    Driver driver;
    KitFixtures fixtures;

    void run() {
        Assert.assertTrue(new Service() != null);
    }
}
//? diag unresolved-import driver
//? diag unresolved-import junit
//? diag unresolved-import fixtures
//? diag unresolved-import kit
//? diag unresolved-import testfixtures
//? diag exact
