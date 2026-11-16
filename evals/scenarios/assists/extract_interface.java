//- /app/UserServiceImpl.java
package app;

import java.util.List;
import java.io.IOException;
import java.util.Map;

public class /*@e1*/UserServiceImpl extends Base {
    private final Map<String, Integer> cache = null;

    /**
     * Finds users.
     */
    public List<String> find(String prefix) throws IOException {
        return List.of(prefix);
    }

    @Override
    public String toString() {
        return "x";
    }

    public static UserServiceImpl create() {
        return new UserServiceImpl();
    }

    public <T> T first(List<T> items) {
        return items.get(0);
    }

    void helper() {
    }
}

//- /app/Base.java
package app;

class Base {
}

//- /app/Box.java
package app;

class /*@e2*/Box<T> implements Comparable<Box<T>> {
    public T get() {
        return null;
    }

    public int compareTo(Box<T> other) {
        return 0;
    }
}

//- /app/Empty.java
package app;

class /*@e3*/Empty {
    void run() {
    }
}

//? assist extract_interface e1 ; has "//- UserService.java\npackage app;\n\nimport java.util.List;\nimport java.io.IOException;\n\npublic interface UserService {\n    /**\n     * Finds users.\n     */\n    List<String> find(String prefix) throws IOException;\n\n    <T> T first(List<T> items);\n}" ; has "public class UserServiceImpl extends Base implements UserService {" ; has "    @Override\n    public List<String> find(" ; has "    @Override\n    public <T> T first(" ; lacks "Map;\n\npublic interface"
//? assist extract_interface e2 ; has "public interface IBox<T> {\n    T get();\n\n    int compareTo(Box<T> other);\n}" ; has "class Box<T> implements Comparable<Box<T>>, IBox<T> {"
//? noassist extract_interface e3
