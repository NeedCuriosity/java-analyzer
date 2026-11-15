//- /lombok/Data.java
package lombok;
public @interface Data {}
//- /lombok/Builder.java
package lombok;
public @interface Builder {}
//- /lombok/Getter.java
package lombok;
public @interface Getter {}
//- /lombok/Setter.java
package lombok;
public @interface Setter {}
//- /lombok/AllArgsConstructor.java
package lombok;
public @interface AllArgsConstructor {}
//- /lombok/NoArgsConstructor.java
package lombok;
public @interface NoArgsConstructor {}
//- /app/User.java
package app;

import lombok.Builder;
import lombok.Data;

@Data
@Builder
public class User {
    private String /*@name*/name;
    private int /*@age*/age;
    private boolean /*@active*/active;
}
//- /app/Point.java
package app;

import lombok.AllArgsConstructor;
import lombok.Getter;
import lombok.NoArgsConstructor;
import lombok.Setter;

@Getter
@AllArgsConstructor
@NoArgsConstructor
public class Point {
    private int x;
    @Setter private int /*@y*/y;
}
//- /app/Main.java
package app;

class Main {
    String run(User user) {
        user./*@set_name*/setName("a");
        user.setAge(user./*@get_age*/getAge() + 1);
        boolean on = user./*@is_active*/isActive();
        User built = User.builder().name("b").age(3).active(on).build();
        Point p = new Point(1, 2);
        Point q = new Point();
        q./*@set_y*/setY(p.getX() + p.getY());
        var /*@n*/n = built.getName();
        return n + user.toString() + user.hashCode() + user.equals(built) + user./*@members*/;
    }
}
//? def set_name -> name
//? def get_age -> age
//? def is_active -> active
//? def set_y -> y
//? hover n ~ "String n"
//? complete members has getName setName getAge setAge isActive
//? complete members lacks getActive setX
