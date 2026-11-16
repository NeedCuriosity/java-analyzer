//- /app/src/main/java/app/Settings.java
package app;

import org.springframework.beans.factory.annotation.Value;
import org.springframework.core.env.Environment;

class Settings {
    @Value("${/*@port*/server.port}")
    int port;

    @Value("${app.max-size:10}")
    int maxSize;

    @Value("${app.servers[0]}")
    String first;

    @Value("${/*@missing*/app.timeout}")
    int timeout;

    @Value("${app.retry:${/*@nested*/app.missing2}}")
    int retry;

    @Value("${JAVA_HOME}")
    String javaHome;

    @Value("${/*@relaxed*/app.mail.host-name}")
    String mailHost;

    @Value("${app.na/*@typing*/:x}")
    String typing;

    String lookup(Environment env) {
        return env.getProperty("/*@env*/app.name");
    }

    String system() {
        return System.getProperty("/*@sys*/app.name");
    }
}

//- /app/src/main/java/org/springframework/beans/factory/annotation/Value.java
package org.springframework.beans.factory.annotation;

public @interface Value {
    String value();
}

//- /app/src/main/java/org/springframework/core/env/Environment.java
package org.springframework.core.env;

public interface Environment {
    String getProperty(String key);
}

//- /app/src/main/resources/application.properties
/*@portdef*/server.port=8080
/*@namedef*/app.name=demo
app.max-size=20

//- /app/src/main/resources/application.yml
app:
  servers:
    - a.example.com
  mail:
    /*@hostdef*/hostName: smtp.example.com
  retry: 3

//? def port -> portdef
//? def env -> namedef
//? def relaxed -> hostdef
//? hover port ~ "server.port = 8080"
//? diag unresolved-config-property missing
//? diag unresolved-config-property nested
//? diag only unresolved-config-property
//? complete typing has "app.name" "app.max-size" "app.servers[0]" "app.mail.hostName" "server.port"
//? complete sys lacks "app.name"
