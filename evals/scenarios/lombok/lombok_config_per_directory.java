//- /lombok.config
# 整个工程
lombok.accessors.chain = true
lombok.accessors.prefix += m
//- /app/fluent/lombok.config
# 叠加在上层的配置上：去掉上层加的前缀
lombok.accessors.fluent = true
lombok.accessors.prefix -= m
//- /app/legacy/lombok.config
# 不再往上找
config.stopBubbling = true
//- /lombok/Getter.java
package lombok;
public @interface Getter {}
//- /lombok/Setter.java
package lombok;
public @interface Setter {}
//- /app/Person.java
package app;

import lombok.Getter;
import lombok.Setter;

@Getter
@Setter
public class Person {
    private String mName;
}
//- /app/fluent/Point.java
package app.fluent;

import lombok.Getter;
import lombok.Setter;

@Getter
@Setter
public class Point {
    private int x;
}
//- /app/legacy/Old.java
package app.legacy;

import lombok.Getter;
import lombok.Setter;

@Getter
@Setter
public class Old {
    private String mName;
}
//- /app/Main.java
package app;

import app.fluent.Point;
import app.legacy.Old;

class Main {
    String run() {
        Person person = new Person().setName("x");
        Point point = new Point().x(1);
        Old old = new Old();
        old.setMName("y");
        old./*@old*/getMName();
        point./*@point*/x();
        return old.getMName() + person.getName() + point.x();
    }
}
//? complete old has getMName setMName
//? complete old lacks getName setName
//? complete point has x
//? complete point lacks getX setX
//? diag exact
