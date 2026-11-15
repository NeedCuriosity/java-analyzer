import java.util.List;

class Person {
    private final String name;
    private final int age;

    public /*@b1*/Person(String name, int age) {
        this.name = name;
        this.age = age;
    }
}

class Box<T extends Comparable<T>> {
    private final T value;
    private final List<T> extras;

    Box(T value, List<T> /*@nob1*/extras) {
        this.value = value;
        this.extras = extras;
    }

    /*@b2*/Box(T value, T... rest) {
        this(value, List.of(rest));
    }
}

record Point(int /*@nob2*/x, int y) {}

record /*@b3*/Pair(String left, String right) {}

abstract class Shape {
    /*@nob3*/Shape(int sides) {}
}
//? assist generate_builder b1 ; has "    public static Builder builder() {\n        return new Builder();\n    }\n\n    public static final class Builder {\n        private String name;\n        private int age;\n\n        private Builder() {\n        }\n\n        public Builder name(String name) {\n            this.name = name;\n            return this;\n        }\n\n        public Builder age(int age) {\n            this.age = age;\n            return this;\n        }\n\n        public Person build() {\n            return new Person(name, age);\n        }\n    }\n}"
//? assist generate_builder b2 ; has "public static <T extends Comparable<T>> Builder<T> builder() {" ; has "public static final class Builder<T extends Comparable<T>> {\n        private T value;\n        private T[] rest;" ; has "public Builder<T> rest(T... rest) {" ; has "return new Box<T>(value, rest);"
//? assist generate_builder b3 ; has "return new Pair(left, right);"
//? noassist generate_builder nob1
//? noassist generate_builder nob2
//? noassist generate_builder nob3
