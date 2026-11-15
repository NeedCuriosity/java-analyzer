import java.util.List;

class Main {
    void a(List<String> names) {
        names./*@coll*/
    }

    void b(String[] parts) {
        parts./*@arr*/
    }

    void c(int n) {
        n./*@int*/
    }

    void d(Day day) {
        day./*@en*/
    }

    void e(String text) {
        text./*@str*/
    }

    enum Day { MON, TUE }
}
//? complete coll has stream opt for notnull try
//? insert coll stream ~ "names.stream()"
//? complete arr has stream for
//? insert arr stream ~ "Arrays.stream(parts)"
//? complete int has forr fori switch
//? insert int forr ~ "for (int ${1:i} = n - 1; $1 >= 0; $1--)"
//? complete en has switch
//? insert str opt ~ "Optional.ofNullable(text)"
