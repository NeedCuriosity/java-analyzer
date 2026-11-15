import java.util.List;

class /*@cls*/Person {
    private final String name;
    private int age;
    private List<String> tags;
}

interface Greeter {
    String greet(String who);
    int priority();
    default void reset() {}
}

class /*@impl*/Polite implements Greeter {
}

class Child extends Person {
    public String /*@override*/toString() { return "child"; }
}
//? assist generate_constructor cls ; has "Person(String name, int age, List<String> tags)" ; has "this.name = name;"
//? assist generate_getters cls ; has "public String getName()" ; has "public int getAge()" ; has "public List<String> getTags()" ; lacks "setName"
//? assist generate_getters_setters cls ; has "public void setAge(int age)" ; has "this.age = age;" ; lacks "setName"
//? assist generate_to_string cls ; has "public String toString()" ; has "@Override"
//? assist generate_equals_hash_code cls ; has "public boolean equals(Object" ; has "public int hashCode()" ; has "Objects.hash(name, age, tags)"
//? assist implement_methods impl ; has "public String greet(String who)" ; has "public int priority()" ; lacks "public void reset"
//? assist add_override override ; has "@Override"
//? noassist implement_methods cls
//? noassist add_override cls
