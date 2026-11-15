//- /app/Animal.java
package app;

public interface /*@animal*/Animal {
    String /*@sound*/sound();
}
//- /app/Dog.java
package app;

public class /*@dog*/Dog implements Animal {
    public String /*@dog_sound*/sound() { return "woof"; }
}
//- /app/Cat.java
package app;

public class /*@cat*/Cat implements Animal {
    public String /*@cat_sound*/sound() { return "meow"; }
}
//- /app/Kitten.java
package app;

public class /*@kitten*/Kitten extends Cat {
    @Override
    public String /*@kitten_sound*/sound() { return "mew"; }
}
//- /app/Main.java
package app;

class Main {
    String run(Animal /*@pet_decl*/pet) { return /*@pet_use*/pet./*@call*/sound(); }
}
//? impl animal -> dog cat kitten
//? impl sound -> dog_sound cat_sound kitten_sound
//? impl cat -> kitten
//? def call -> sound
//? typedef pet_use -> animal
//? diag none
