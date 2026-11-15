import java.util.ArrayList;
import java.util.List;

class Main {
    void run() {
        List<String> xs = new /*@ctor*/ArrayList<>();
        xs./*@add*/add("a");
        String s = /*@string*/String./*@value_of*/valueOf(1);
        Math./*@max*/max(1, 2);
    }
}
//? hover add ~ "boolean add(E"
//? hover value_of ~ "String valueOf(int"
//? hover max ~ "int max(int"
//? hover string ~ "class String"
//? hover string ~ "java.lang"
