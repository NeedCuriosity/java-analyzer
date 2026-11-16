//- /src/main/java/app/AppProperties.java
package app;

import java.time.Duration;
import java.util.List;
import java.util.Map;

import org.springframework.boot.context.properties.ConfigurationProperties;

@ConfigurationProperties(prefix = "app")
public class AppProperties {
    /** Maximum number of open connections. */
    private int /*@maxdef*/maxConnections = 10;
    private boolean enabled;
    private Mode mode = Mode.FAST;
    private Duration timeout;
    private List<String> servers;
    private Map<String, String> labels;
    private final Mail mail = new Mail();

    public enum Mode { FAST, SAFE_AND_SLOW }

    public static class /*@maildef*/Mail {
        private String /*@hostdef*/hostName;
        private int port = 25;
    }
}

//- /src/main/java/app/Client.java
package app;

import org.springframework.beans.factory.annotation.Value;

class Client {
    @Value("${app.mail./*@valuekey*/}")
    String host;
}

//- /src/main/java/org/springframework/boot/context/properties/ConfigurationProperties.java
package org.springframework.boot.context.properties;

public @interface ConfigurationProperties {
    String value() default "";
    String prefix() default "";
}

//- /src/main/java/org/springframework/beans/factory/annotation/Value.java
package org.springframework.beans.factory.annotation;

public @interface Value {
    String value();
}

//- /src/main/resources/application.properties
app.max-con/*@pmax*/nections=20
app.ma/*@pkey*/
app.enabled=/*@pbool*/
app.mode=s/*@penum*/
app.ser/*@unknown*/vers2=x
app.labels.team=core
app.servers[0]=a
custom.thing=1
app.mail.host-name/*@phover*/=smtp

//- /src/main/resources/application.yml
app:
  ma/*@yhover*/il:
    /*@ykey*/
  mode: /*@yenum*/
  time/*@ypartial*/
  /*@ybogus*/bogus: 1

//- /src/main/resources/application-dev.yml
ap/*@yroot*/

//? complete pkey has "app.max-connections" "app.mail.host-name"
//? complete pkey lacks "app.mode"
//? insert pkey app.max-connections ~ "app.max-connections=" plain
//? complete pbool has "true" "false"
//? complete penum has "fast" "safe-and-slow"
//? hover pmax ~ "int app.max-connections = 10"
//? hover phover ~ "String app.mail.host-name"
//? def pmax -> maxdef
//? def phover -> hostdef
//? diag unknown-config-property pkey
//? diag unknown-config-property unknown
//? diag unknown-config-property ybogus
//? diag only unknown-config-property
//? complete ykey has "host-name" "port"
//? complete ykey lacks "mode"
//? insert ykey port ~ "port: " plain
//? complete yenum has "fast" "safe-and-slow"
//? complete ypartial has "timeout"
//? complete ypartial lacks "mail" "mode"
//? insert yroot app ~ "app:\n  " plain
//? hover yhover ~ "AppProperties.Mail app.mail"
//? def yhover -> maildef
//? complete valuekey has "app.mail.port" "app.max-connections"
