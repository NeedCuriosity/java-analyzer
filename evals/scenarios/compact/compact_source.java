//- /compact/Main2.java
int count = 0;

void main() {
    String s = "x";
    s./*@typo*/lenght();
    /*@undefined*/undefinedCall();
    count = /*@mismatch*/"not an int";
}
//- /compact/Main.java
String /*@greetdecl*/greet(String name) { return "Hello " + name; }

record Pair(String a, String b) {}

void main() {
    List<String> /*@xs*/xs = List.of("a");
    IO.println(/*@greet*/greet(xs.get(0)).trim());
    Pair p = new Pair("a", "b");
    IO.println(p.a() + this.greet(p.b()));
    var /*@map*/map = new HashMap<String, Integer>();
    map.put(p.a(), 1);
}
//? diag unresolved-method typo
//? diag unresolved-method undefined
//? diag type-mismatch mismatch
//? hover xs ~ "List<String> xs"
//? hover map ~ "HashMap<String, Integer> map"
//? def greet -> greetdecl
//? diag exact
