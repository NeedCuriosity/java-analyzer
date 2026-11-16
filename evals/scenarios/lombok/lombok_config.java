//- /lombok.config
# 整个工程的 Lombok 配置
lombok.accessors.chain = true
lombok.accessors.prefix += m
lombok.getter.noIsPrefix = true
lombok.log.fieldName = LOGGER
//- /lombok/Getter.java
package lombok;
public @interface Getter {}
//- /lombok/Setter.java
package lombok;
public @interface Setter {}
//- /lombok/extern/slf4j/Slf4j.java
package lombok.extern.slf4j;
public @interface Slf4j {}
//- /lombok/experimental/Accessors.java
package lombok.experimental;
public @interface Accessors { boolean chain() default false; boolean fluent() default false; String[] prefix() default {}; }
//- /org/slf4j/Logger.java
package org.slf4j;
public interface Logger { void info(String message); }
//- /app/Person.java
package app;

import lombok.Getter;
import lombok.Setter;

@Getter
@Setter
public class Person {
    private String mName;
    private boolean mActive;
}
//- /app/Plain.java
package app;

import lombok.Getter;
import lombok.Setter;
import lombok.experimental.Accessors;

// 注解上写了的参数优先于 lombok.config
@Getter
@Setter
@Accessors(chain = false, prefix = {})
public class Plain {
    private String mName;
}
//- /app/Main.java
package app;

import lombok.extern.slf4j.Slf4j;

@Slf4j
class Main {
    void run() {
        Person p = new Person().setName("x").setActive(true);
        boolean active = p.getActive();
        String name = p.getName();
        Plain plain = new Plain();
        plain.setMName("y");
        String other = plain.getMName();
        LOGGER.info(name + active + other);
        p./*@members*/getName();
    }
}

//? diag exact
//? complete members has getName setName getActive setActive
//? complete members lacks getMName isActive
