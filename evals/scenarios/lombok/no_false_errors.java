//- /lombok/Data.java
package lombok;
public @interface Data {}
//- /lombok/Builder.java
package lombok;
public @interface Builder {}
//- /lombok/Value.java
package lombok;
public @interface Value {}
//- /lombok/RequiredArgsConstructor.java
package lombok;
public @interface RequiredArgsConstructor {}
//- /lombok/extern/slf4j/Slf4j.java
package lombok.extern.slf4j;
public @interface Slf4j {}
//- /org/slf4j/Logger.java
package org.slf4j;
public interface Logger { void info(String message); }
//- /app/Config.java
package app;

import lombok.Value;

@Value
public class Config {
    String host;
    int port;
}
//- /app/Service.java
package app;

import lombok.RequiredArgsConstructor;
import lombok.extern.slf4j.Slf4j;

@Slf4j
@RequiredArgsConstructor
public class Service {
    private final Config config;
    private int calls;

    String address() {
        calls++;
        log.info("address");
        return config.getHost() + ":" + config.getPort();
    }

    static Service create() {
        return new Service(new Config("localhost", 80));
    }
}
//? diag none
