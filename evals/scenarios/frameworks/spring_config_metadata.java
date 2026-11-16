//- /src/main/java/app/App.java
package app;

class App {
}

//- /META-INF/spring-configuration-metadata.json lib:boot
{
  "groups": [
    {
      "name": "server",
      "type": "org.example.boot.ServerProperties",
      "sourceType": "org.example.boot.ServerProperties"
    }
  ],
  "properties": [
    {
      "name": "server.port",
      "type": "java.lang.Integer",
      "description": "Server HTTP port.",
      "sourceType": "org.example.boot.ServerProperties",
      "defaultValue": 8080
    },
    {
      "name": "server.shutdown",
      "type": "org.example.boot.Shutdown",
      "sourceType": "org.example.boot.ServerProperties",
      "defaultValue": "immediate"
    },
    {
      "name": "server.legacy-timeout",
      "type": "java.time.Duration",
      "deprecated": true,
      "deprecation": { "level": "error", "replacement": "server.port" }
    },
    {
      "name": "server.compression.enabled",
      "type": "java.lang.Boolean",
      "defaultValue": false
    },
    {
      "name": "logging.level",
      "type": "java.util.Map<java.lang.String,java.lang.String>"
    },
    {
      "name": "spring.jpa.hibernate.ddl-auto",
      "type": "java.lang.String"
    }
  ],
  "hints": [
    {
      "name": "logging.level.values",
      "values": [{ "value": "info" }, { "value": "debug" }]
    },
    {
      "name": "spring.jpa.hibernate.ddl-auto",
      "values": [
        { "value": "none", "description": "Disable DDL handling." },
        { "value": "update" }
      ]
    }
  ]
}

//- /org/example/boot/ServerProperties.java lib:boot
package org.example.boot;

public class ServerProperties {
    private Integer port;
    private Shutdown shutdown;

    public void setPort(Integer port) {
        this.port = port;
    }
}

//- /org/example/boot/Shutdown.java lib:boot
package org.example.boot;

public enum Shutdown { GRACEFUL, IMMEDIATE }

//- /src/main/resources/application.properties
server.po/*@k*/
server.shutdown=/*@shut*/
server.legacy-time/*@dep*/out=10s
server.prt/*@typo*/=1
logging.level.org.example=/*@level*/
spring.jpa.hibernate.ddl-auto=/*@ddl*/
my.custom.key=1
server.port/*@hov*/=9090

//- /src/main/resources/application.yaml
server:
  compression:
    /*@ycomp*/
  /*@yserver*/

//? complete k has "server.port"
//? complete k lacks "server.shutdown" "logging.level"
//? insert k server.port ~ "server.port=" plain
//? complete shut has "graceful" "immediate"
//? complete level has "info" "debug"
//? complete ddl has "none" "update"
//? diag deprecated-config-property dep
//? diag unknown-config-property k
//? diag unknown-config-property typo
//? diag only unknown-config-property
//? hover hov ~ "Integer server.port = 8080"
//? def hov -> lib org.example.boot.ServerProperties#port
//? complete ycomp has "enabled"
//? complete yserver has "port" "shutdown" "legacy-timeout"
//? complete yserver lacks "compression"
