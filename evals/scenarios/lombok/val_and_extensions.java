//- /lombok/val.java
package lombok;
public final class val { private val() {} }
//- /lombok/Data.java
package lombok;
public @interface Data {}
//- /lombok/Getter.java
package lombok;
public @interface Getter {}
//- /lombok/Setter.java
package lombok;
public @interface Setter {}
//- /lombok/experimental/Accessors.java
package lombok.experimental;
public @interface Accessors { boolean chain() default false; boolean fluent() default false; }
//- /lombok/experimental/SuperBuilder.java
package lombok.experimental;
public @interface SuperBuilder {}
//- /lombok/experimental/Delegate.java
package lombok.experimental;
public @interface Delegate {}
//- /lombok/experimental/UtilityClass.java
package lombok.experimental;
public @interface UtilityClass {}
//- /app/Point.java
package app;

import lombok.Data;
import lombok.experimental.Accessors;

@Data
@Accessors(chain = true)
public class Point {
    private int x;
    private int y;
}
//- /app/Fluent.java
package app;

import lombok.Getter;
import lombok.Setter;
import lombok.experimental.Accessors;

@Getter
@Setter
@Accessors(fluent = true)
public class Fluent {
    private String name;
}
//- /app/Animal.java
package app;

import lombok.experimental.SuperBuilder;

@SuperBuilder
public class Animal {
    private String name;
}
//- /app/Dog.java
package app;

import lombok.experimental.SuperBuilder;

@SuperBuilder
public class Dog extends Animal {
    private int bark;
}
//- /app/Names.java
package app;

import java.util.ArrayList;
import java.util.List;
import lombok.experimental.Delegate;

public class Names {
    @Delegate
    private final List<String> items = new ArrayList<>();
}
//- /app/Strings.java
package app;

import lombok.experimental.UtilityClass;

@UtilityClass
public class Strings {
    public String twice(String s) {
        return s + s;
    }
}
//- /app/Main.java
package app;

import java.util.ArrayList;
import java.util.List;
import lombok.val;

class Main {
    void run() {
        val /*@list*/list = new ArrayList<String>();
        list.add("a");
        val /*@first*/first = list.get(0);
        for (val /*@item*/item : list) {
            item.length();
        }
        /*@reassign*/first = "b";

        Point point = new Point().setX(1).setY(2);
        Fluent fluent = new Fluent().name("n");
        String name = fluent.name();

        Dog dog = Dog.builder().name("rex").bark(3).build();

        Names names = new Names();
        names.add("x");
        String /*@got*/got = names.get(0);
        int size = names.size();

        String twice = Strings.twice("ab");
    }
}
//? hover list ~ "ArrayList<String> list"
//? hover first ~ "String first"
//? hover item ~ "String item"
//? diag assign-to-final reassign
//? diag exact
