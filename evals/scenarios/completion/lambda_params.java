import java.util.List;
import java.util.function.Function;

class Main {
    Function<String, Integer> build() {
        return text -> /*@body*/.length();
    }

    void run(List<String> names) {
        names.forEach(name -> System.out.println(/*@arg*/));
    }
}
//? complete body first text
//? complete arg has name
